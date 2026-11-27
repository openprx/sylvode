#!/bin/sh
# Acceptance suite for deploy/tls (sylvode-tls.sh, trust-client.sh).
#
# Meant for a disposable Debian 13 container. It installs packages and creates users, so it
# refuses to run unless SYLVODE_TLS_ACCEPTANCE=1 and the user is root:
#
#   podman run --rm -v <repo>:/src:ro docker.io/library/debian:trixie \
#     sh -c 'SYLVODE_TLS_ACCEPTANCE=1 sh /src/deploy/tls/tests/acceptance.sh'
#
# Prints "PASS T-nn <title>" or "FAIL T-nn <title>: <details>" per test, saves every command's
# output to /tmp/acceptance/log and exits non-zero when a test failed. The state directories
# live under /opt/tlswork (not /tmp): the scripts refuse sticky or world-writable directories.

if [ "${SYLVODE_TLS_ACCEPTANCE:-}" != 1 ]; then
  echo "refusing to run: set SYLVODE_TLS_ACCEPTANCE=1 (this suite installs packages and creates users)" >&2
  exit 2
fi
if [ "$(id -u)" != 0 ]; then
  echo "refusing to run: root is required" >&2
  exit 2
fi

SRC=$(cd "$(dirname "$0")/.." && pwd)
LOGDIR=/tmp/acceptance
LOG=$LOGDIR/log
KEYLINES=$LOGDIR/keylines
W=/opt/tlswork
D=$W/state
OUTD=$D/server
BG_PIDS=
FAILS=0
TOTAL=0
RC=0
WHY=
T_OK=1
HINT_FILE=$LOGDIR/hint

mkdir -p "$LOGDIR" /opt/tls "$W"
chmod 0755 "$W" /opt/tls
: >"$LOG"
: >"$KEYLINES"

cleanup() {
  for _p in $BG_PIDS; do
    kill "$_p" 2>/dev/null
  done
  if [ -f /run/nginx.pid ]; then
    nginx -s stop 2>/dev/null
  fi
}
trap cleanup EXIT

pass() {
  printf 'PASS %s\n' "$*"
}

fail() {
  printf 'FAIL %s\n' "$*"
  printf '  --- last log lines ---\n'
  tail -n 25 "$LOG" | sed 's/^/  | /'
}

# runlog CMD...: run, append the command and its output to the log; sets RC.
runlog() {
  "$@" >"$LOGDIR/o" 2>"$LOGDIR/e"
  RC=$?
  {
    printf '$ %s\n' "$*"
    cat "$LOGDIR/o" "$LOGDIR/e"
    printf '[exit %s]\n' "$RC"
  } >>"$LOG"
}

collect_keys() {
  for _k in "$D/ca/ca.key" "$D/next/ca.key" "$OUTD/server.key" "$W/ipo/ca/ca.key"; do
    if [ -f "$_k" ]; then
      grep -v -e '^-----' "$_k" | grep -E '.{16,}' >>"$KEYLINES"
    fi
  done
  return 0
}

tls() {
  runlog dash /opt/tls/sylvode-tls.sh "$@"
  collect_keys
}

trust() {
  runlog dash /opt/tls/trust-client.sh "$@"
}

# ck DESCRIPTION CMD...: a failed check marks the current test as failed.
ck() {
  _d=$1
  shift
  if ! "$@" >/dev/null 2>&1; then
    WHY="${WHY}${WHY:+; }$_d"
    T_OK=0
  fi
}

run_test() {
  _id=$1
  _title=$2
  _fn=$3
  WHY=
  T_OK=1
  TOTAL=$((TOTAL + 1))
  "$_fn"
  if [ "$T_OK" = 1 ]; then
    pass "$_id $_title"
  else
    FAILS=$((FAILS + 1))
    fail "$_id $_title: $WHY"
  fi
}

skip() {
  printf 'SKIP %s\n' "$*"
}

h() {
  sha256sum "$1" | cut -d' ' -f1
}

perm() {
  stat -c '%a %U' "$1"
}

serial_of() {
  openssl x509 -in "$1" -noout -serial
}

pub_of() {
  openssl x509 -in "$1" -noout -pubkey
}

fp_of() {
  openssl x509 -in "$1" -noout -fingerprint -sha256 | sed 's/^.*Fingerprint=//'
}

out_has() {
  grep -q -e "$1" "$LOGDIR/o"
}

err_has() {
  grep -q -e "$1" "$LOGDIR/e"
}

serve() {
  # serve HOST:PORT CERT KEY CHAIN: start openssl s_server in the background.
  openssl s_server -accept "$1" -cert "$2" -key "$3" -cert_chain "$4" -www >/dev/null 2>&1 &
  SERVER_PID=$!
  BG_PIDS="$BG_PIDS $SERVER_PID"
  sleep 1
}

unserve() {
  kill "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
}

# mint CADIR ID SUBJECT SAN: leaf signed by hand with the CA key (bypassing the script).
mint() {
  _m=$W/mint-$2
  mkdir -p "$_m"
  openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$_m/k.pem" || return 1
  openssl req -new -key "$_m/k.pem" -subj "$3" -out "$_m/c.csr" || return 1
  {
    printf '[leaf]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n'
    if [ -n "$4" ]; then
      printf 'subjectAltName=%s\n' "$4"
    fi
  } >"$_m/l.cnf"
  openssl x509 -req -in "$_m/c.csr" -CA "$1/ca.crt" -CAkey "$1/ca/ca.key" -set_serial "0x$(openssl rand -hex 8)" \
    -days 5 -sha256 -extfile "$_m/l.cnf" -extensions leaf -out "$_m/l.crt" 2>/dev/null
}

# ---------------------------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------------------------

echo "== setup: installing packages"
export DEBIAN_FRONTEND=noninteractive
if ! { apt-get update -qq && apt-get install -y -qq --no-install-recommends openssl curl nginx shellcheck dash \
  ca-certificates libnss3-tools procps util-linux passwd >"$LOGDIR/apt.log" 2>&1; }; then
  echo "setup failed: apt-get could not install the packages (see $LOGDIR/apt.log)" >&2
  tail -n 20 "$LOGDIR/apt.log" >&2
  exit 1
fi
cp "$SRC/sylvode-tls.sh" "$SRC/trust-client.sh" /opt/tls/
chmod 0755 /opt/tls/sylvode-tls.sh /opt/tls/trust-client.sh
openssl version
echo "== running"

# ---------------------------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------------------------

t_01() {
  runlog shellcheck -s sh "$SRC/sylvode-tls.sh" "$SRC/trust-client.sh" "$SRC/tests/acceptance.sh"
  ck "shellcheck reported findings (exit $RC)" [ "$RC" = 0 ]
  runlog shellcheck -s sh -S style "$SRC/sylvode-tls.sh" "$SRC/trust-client.sh" "$SRC/tests/acceptance.sh"
  ck "shellcheck -S style reported findings (exit $RC)" [ "$RC" = 0 ]
}

t_02() {
  tls init-ca --dir "$D" --ip 127.0.0.1 --dns sylvode.internal
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  nc=$(openssl x509 -in "$D/ca.crt" -noout -ext nameConstraints)
  ck "nameConstraints is critical" sh -c "printf '%s\n' \"\$1\" | grep -q 'Name Constraints: critical'" _ "$nc"
  entries=$(printf '%s\n' "$nc" | grep -E '^ +(IP|DNS):' | sed 's/^ *//' | tr '\n' ' ')
  ck "permitted subtrees are exactly IP:127.0.0.1/255.255.255.255 and DNS:sylvode.internal (got: $entries)" \
    [ "$entries" = "IP:127.0.0.1/255.255.255.255 DNS:sylvode.internal " ]
  bc=$(openssl x509 -in "$D/ca.crt" -noout -ext basicConstraints | tr '\n' ' ')
  ck "basicConstraints critical CA:TRUE pathlen:0 (got: $bc)" sh -c "printf '%s' \"\$1\" | grep -q 'critical' && printf '%s' \"\$1\" | grep -q 'CA:TRUE, pathlen:0'" _ "$bc"
  ku=$(openssl x509 -in "$D/ca.crt" -noout -ext keyUsage | tr '\n' ' ')
  ck "keyUsage critical keyCertSign, cRLSign (got: $ku)" sh -c "printf '%s' \"\$1\" | grep -q 'critical' && printf '%s' \"\$1\" | grep -q 'Certificate Sign, CRL Sign'" _ "$ku"
  eku=$(openssl x509 -in "$D/ca.crt" -noout -ext extendedKeyUsage | tr '\n' ' ')
  ck "extendedKeyUsage serverAuth (got: $eku)" sh -c "printf '%s' \"\$1\" | grep -q 'TLS Web Server Authentication'" _ "$eku"
}

t_03() {
  ck "$D is 755 root (got $(perm "$D"))" [ "$(perm "$D")" = "755 root" ]
  ck "$D/ca is 700 root (got $(perm "$D/ca"))" [ "$(perm "$D/ca")" = "700 root" ]
  ck "ca.key is 400 root (got $(perm "$D/ca/ca.key"))" [ "$(perm "$D/ca/ca.key")" = "400 root" ]
  ck "ca.crt is 644 (got $(perm "$D/ca.crt"))" [ "$(perm "$D/ca.crt" | cut -d' ' -f1)" = 644 ]
}

t_04() {
  k0=$(h "$D/ca/ca.key")
  c0=$(h "$D/ca.crt")
  tls init-ca --dir "$D" --dns sylvode.internal --ip 127.0.0.1
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "ca.key unchanged" [ "$(h "$D/ca/ca.key")" = "$k0" ]
  ck "ca.crt unchanged" [ "$(h "$D/ca.crt")" = "$c0" ]
}

t_05() {
  k0=$(h "$D/ca/ca.key")
  c0=$(h "$D/ca.crt")
  tls init-ca --dir "$D" --ip 127.0.0.1
  ck "exit 3 (got $RC)" [ "$RC" = 3 ]
  ck "ca.key unchanged" [ "$(h "$D/ca/ca.key")" = "$k0" ]
  ck "ca.crt unchanged" [ "$(h "$D/ca.crt")" = "$c0" ]
}

t_06() {
  useradd -r svc >/dev/null 2>&1
  tls issue --dir "$D" --owner svc:svc
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "server.key is 600 svc (got $(perm "$OUTD/server.key"))" [ "$(perm "$OUTD/server.key")" = "600 svc" ]
  ck "server.crt is 644 root (got $(perm "$OUTD/server.crt"))" [ "$(perm "$OUTD/server.crt")" = "644 root" ]
  ck "fullchain.pem is 644 root (got $(perm "$OUTD/fullchain.pem"))" [ "$(perm "$OUTD/fullchain.pem")" = "644 root" ]
  subj=$(openssl x509 -in "$OUTD/server.crt" -noout -subject)
  ck "leaf has no CN (got $subj)" sh -c "! printf '%s' \"\$1\" | grep -q 'CN'" _ "$subj"
  san=$(openssl x509 -in "$OUTD/server.crt" -noout -ext subjectAltName | tr '\n' ' ')
  ck "SAN is IP:127.0.0.1, DNS:sylvode.internal (got $san)" sh -c "printf '%s' \"\$1\" | grep -q 'IP Address:127.0.0.1' && printf '%s' \"\$1\" | grep -q 'DNS:sylvode.internal'" _ "$san"
  ck "SAN is not critical" sh -c "! printf '%s' \"\$1\" | grep -q critical" _ "$san"
  ck "EKU serverAuth" sh -c "openssl x509 -in $OUTD/server.crt -noout -ext extendedKeyUsage | grep -q 'TLS Web Server Authentication'"
  ck "keyUsage critical digitalSignature" sh -c "openssl x509 -in $OUTD/server.crt -noout -ext keyUsage | tr '\n' ' ' | grep -q 'critical.*Digital Signature'"
  ck "CA:FALSE" sh -c "openssl x509 -in $OUTD/server.crt -noout -ext basicConstraints | grep -q 'CA:FALSE'"
  ck "valid for at least 396 days" openssl x509 -in "$OUTD/server.crt" -noout -checkend $((396 * 86400))
  ck "valid for less than 398 days" sh -c "! openssl x509 -in $OUTD/server.crt -noout -checkend $((398 * 86400))"
  ck "verify -purpose sslserver" openssl verify -CAfile "$D/ca.crt" -purpose sslserver "$OUTD/server.crt"
  ck "fullchain.pem is leaf plus CA" sh -c "cat $OUTD/server.crt $D/ca.crt | cmp -s - $OUTD/fullchain.pem"
}

t_07() {
  s0=$(serial_of "$OUTD/server.crt")
  tls issue --dir "$D"
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "serial unchanged" [ "$(serial_of "$OUTD/server.crt")" = "$s0" ]
}

t_08() {
  s0=$(serial_of "$OUTD/server.crt")
  p0=$(pub_of "$OUTD/server.crt")
  tls issue --dir "$D" --force
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "new serial" [ "$(serial_of "$OUTD/server.crt")" != "$s0" ]
  ck "new public key" [ "$(pub_of "$OUTD/server.crt")" != "$p0" ]
}

t_09() {
  c0=$(h "$OUTD/server.crt")
  tls issue --dir "$D" --dns evil.example
  ck "issue --dns evil.example exits 3 (got $RC)" [ "$RC" = 3 ]
  tls issue --dir "$D" --ip 127.0.0.2
  ck "issue --ip 127.0.0.2 exits 3 (got $RC)" [ "$RC" = 3 ]
  ck "server.crt unchanged" [ "$(h "$OUTD/server.crt")" = "$c0" ]
  left=$(find "$D" -name '.tmp.*' -o -name '.lock' | tr '\n' ' ')
  ck "no temporary directory or lock left (found: $left)" [ -z "$left" ]
}

t_10() {
  # (a) foreign IP, (b) foreign DNS name, (c) SAN-less leaf with a hostname CN
  mint "$D" a "/O=x" "IP:127.0.0.2" || ck "minting leaf (a)" false
  mint "$D" b "/O=x" "DNS:evil.example" || ck "minting leaf (b)" false
  mint "$D" c "/CN=evil.example" "" || ck "minting leaf (c)" false
  for _l in a b c; do
    runlog openssl verify -CAfile "$D/ca.crt" "$W/mint-$_l/l.crt"
    ck "($_l) verify fails with permitted subtree violation" grep -q 'permitted subtree violation' "$LOGDIR/o" "$LOGDIR/e"
  done
  serve 127.0.0.2:8444 "$W/mint-a/l.crt" "$W/mint-a/k.pem" "$D/ca.crt"
  runlog curl -sS -o /dev/null --cacert "$D/ca.crt" https://127.0.0.2:8444/
  ck "(a) curl exits 60 (got $RC)" [ "$RC" = 60 ]
  unserve
  for _l in b c; do
    serve 127.0.0.1:8444 "$W/mint-$_l/l.crt" "$W/mint-$_l/k.pem" "$D/ca.crt"
    runlog curl -sS -o /dev/null --cacert "$D/ca.crt" --resolve evil.example:8444:127.0.0.1 https://evil.example:8444/
    ck "($_l) curl exits 60 (got $RC)" [ "$RC" = 60 ]
    unserve
  done
}

t_11() {
  serve 127.0.0.1:8443 "$OUTD/server.crt" "$OUTD/server.key" "$D/ca.crt"
  runlog curl -fsS -o /dev/null --cacert "$D/ca.crt" https://127.0.0.1:8443/
  ck "curl by IP exits 0 (got $RC)" [ "$RC" = 0 ]
  runlog curl -fsS -o /dev/null --cacert "$D/ca.crt" --resolve sylvode.internal:8443:127.0.0.1 https://sylvode.internal:8443/
  ck "curl by name exits 0 (got $RC)" [ "$RC" = 0 ]
  unserve
}

t_12() {
  tls config-hint --dir "$D" --proxy nginx --port 443 --http-port 80 --upstream 127.0.0.1:3000
  ck "config-hint exits 0 (got $RC)" [ "$RC" = 0 ]
  cp "$LOGDIR/o" "$HINT_FILE"
  sed -n '/^# ---- nginx/,$p' "$HINT_FILE" >/etc/nginx/conf.d/sylvode-tls.conf
  printf 'server { listen 127.0.0.1:3000; location / { return 200 "ok\\n"; } }\n' >/etc/nginx/conf.d/upstream.conf
  rm -f /etc/nginx/sites-enabled/default
  runlog nginx -t
  ck "nginx -t exits 0 (got $RC)" [ "$RC" = 0 ]
  runlog nginx
  sleep 1
  runlog curl -fsS --cacert "$D/ca.crt" https://127.0.0.1/
  ck "https answers ok (got exit $RC, body $(cat "$LOGDIR/o"))" [ "$(cat "$LOGDIR/o")" = ok ]
  runlog curl -sI http://127.0.0.1/x
  ck "http answers 308" out_has '^HTTP/[0-9.]* 308'
  ck "redirect Location is https://127.0.0.1/x" grep -qi '^location: https://127.0.0.1/x' "$LOGDIR/o"
  runlog curl -sS -o /dev/null https://127.0.0.1/
  ck "https without --cacert exits 60 (got $RC)" [ "$RC" = 60 ]
}

t_13() {
  tls issue --dir "$D" --force --days 10
  ck "issue --force --days 10 exits 0 (got $RC)" [ "$RC" = 0 ]
  s0=$(serial_of "$OUTD/server.crt")
  rm -f /tmp/hook
  tls renew --dir "$D" --threshold-days 5 --exec 'touch /tmp/hook'
  ck "renew exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "serial unchanged" [ "$(serial_of "$OUTD/server.crt")" = "$s0" ]
  ck "/tmp/hook absent" [ ! -e /tmp/hook ]
  tls renew --dir "$D" --check --threshold-days 5
  ck "renew --check exits 0 (got $RC)" [ "$RC" = 0 ]
}

t_14() {
  # The certificate of T-13 lives 10 days, so --days 397 is given: the threshold must stay
  # lower than the lifetime (contract 5.3).
  s0=$(serial_of "$OUTD/server.crt")
  tls renew --dir "$D" --check --threshold-days 30 --days 397
  ck "renew --check exits 10 (got $RC)" [ "$RC" = 10 ]
  ck "serial unchanged by --check" [ "$(serial_of "$OUTD/server.crt")" = "$s0" ]
  rm -f /tmp/hook
  tls renew --dir "$D" --threshold-days 30 --days 397 --exec 'nginx -s reload && touch /tmp/hook'
  ck "renew exits 0 (got $RC)" [ "$RC" = 0 ]
  s1=$(serial_of "$OUTD/server.crt")
  ck "new serial" [ "$s1" != "$s0" ]
  ck "/tmp/hook present" [ -e /tmp/hook ]
  sleep 1
  served=$(openssl s_client -connect 127.0.0.1:443 </dev/null 2>/dev/null | openssl x509 -noout -serial)
  ck "nginx serves the new serial (served $served, expected $s1)" [ "$served" = "$s1" ]
}

t_15() {
  s0=$(serial_of "$OUTD/server.crt")
  tls renew --dir "$D" --force --exec false
  ck "renew --force --exec false exits 5 (got $RC)" [ "$RC" = 5 ]
  ck "new certificate in place" [ "$(serial_of "$OUTD/server.crt")" != "$s0" ]
  ck ".reload-pending present" [ -e "$OUTD/.reload-pending" ]
  tls renew --dir "$D" --threshold-days 30 --exec true
  ck "renew with a pending hook exits 0 (got $RC)" [ "$RC" = 0 ]
  ck ".reload-pending removed" [ ! -e "$OUTD/.reload-pending" ]
}

t_16() {
  chmod 0444 "$D/ca/ca.key"
  tls issue --dir "$D" --force
  ck "exit 3 for a 0444 key (got $RC)" [ "$RC" = 3 ]
  ck "message names ca.key" err_has 'ca\.key'
  chmod 0400 "$D/ca/ca.key"
  chmod 0755 "$D/ca"
  tls issue --dir "$D" --force
  ck "exit 3 for a 0755 ca directory (got $RC)" [ "$RC" = 3 ]
  chmod 0700 "$D/ca"
}

t_18() {
  useradd -m u >/dev/null 2>&1
  tls issue --dir "$D" --out /home/u/tls
  ck "exit 3 (got $RC)" [ "$RC" = 3 ]
  ck "nothing created under /home/u/tls" [ ! -e /home/u/tls ]
  left=$(find /home/u -name '.tmp.*' | tr '\n' ' ')
  ck "no temporary directory under /home/u (found: $left)" [ -z "$left" ]
}

t_19() {
  runlog runuser -u svc -- dash /opt/tls/sylvode-tls.sh issue --dir /tmp/svcstate --owner root
  ck "exit 4 (got $RC)" [ "$RC" = 4 ]
}

t_20() {
  mkdir "$D/.lock"
  echo $$ >"$D/.lock/pid"
  tls issue --dir "$D" --force
  ck "live pid: exit 6 (got $RC)" [ "$RC" = 6 ]
  dead=$(sh -c 'echo $$')
  echo "$dead" >"$D/.lock/pid"
  tls issue --dir "$D" --force
  ck "dead pid: exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "lock released" [ ! -e "$D/.lock" ]
}

t_22() {
  tls fingerprint --dir "$D"
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  want=$(fp_of "$D/ca.crt")
  ck "one line" [ "$(wc -l <"$LOGDIR/o")" = 1 ]
  ck "equals the openssl fingerprint ($want)" out_has "SHA-256 $want "
}

t_23() {
  tls config-hint --dir "$D"
  ck "exit 0 (got $RC)" [ "$RC" = 0 ]
  ck "public_url" out_has 'public_url = "https://127.0.0.1"'
  ck "collab_allowed_origins" out_has 'collab_allowed_origins = \["https://127.0.0.1", "https://sylvode.internal"\]'
  ck "redaction log_format" out_has 'log_format sylvode_tls_redacted'
  ck "fingerprint" out_has "$(fp_of "$D/ca.crt")"
  ck "no PRIVATE" sh -c "! grep -q PRIVATE $LOGDIR/o"
}

t_21() {
  oldkey=$(h "$D/ca/ca.key")
  oldcrt=$(h "$D/ca.crt")
  tls rotate-ca --dir "$D" --dns sylvode.internal --ip 127.0.0.1 --ip 127.0.0.3
  ck "rotate-ca exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "current CA key unchanged" [ "$(h "$D/ca/ca.key")" = "$oldkey" ]
  ck "current CA certificate unchanged" [ "$(h "$D/ca.crt")" = "$oldcrt" ]
  ck "next.crt exists" [ -f "$D/next.crt" ]
  newkey=$(h "$D/next/ca.key")
  tls rotate-ca --dir "$D"
  ck "second rotate-ca exits 3 (got $RC)" [ "$RC" = 3 ]
  tls rotate-ca --dir "$D" --promote
  ck "--promote exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "retired certificate exists" sh -c "ls $D/retired/*.crt"
  leaked=$(find "$D" -type f -exec sha256sum {} + | grep "$oldkey")
  ck "the old key exists nowhere under $D" [ -z "$leaked" ]
  ck "ca.key is the former next key" [ "$(h "$D/ca/ca.key")" = "$newkey" ]
  ck "next directory gone" [ ! -e "$D/next" ]
  s0=$(serial_of "$OUTD/server.crt")
  tls renew --dir "$D" --exec true
  ck "renew exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "renew reissued the leaf" [ "$(serial_of "$OUTD/server.crt")" != "$s0" ]
  ck "new leaf verifies against the new CA" openssl verify -CAfile "$D/ca.crt" -purpose sslserver "$OUTD/server.crt"
  ck "new leaf does not verify against the retired CA" sh -c "! openssl verify -CAfile $D/retired/*.crt $OUTD/server.crt"
  tls issue --dir "$D" --ip 127.0.0.3 --force
  ck "issue --ip 127.0.0.3 --force exits 0 (got $RC)" [ "$RC" = 0 ]
}

t_17() {
  ck "no PRIVATE KEY in any output" sh -c "! grep -q 'PRIVATE KEY' $LOG"
  ck "keys were collected" [ -s "$KEYLINES" ]
  ck "no key body line in any output" sh -c "! grep -F -x -f $KEYLINES $LOG"
}

t_24() {
  # Setup: the T-21 leaf only carries 127.0.0.3; issue the full set again and reload nginx.
  tls issue --dir "$D" --force --exec 'nginx -s reload'
  sleep 1
  fp=$(fp_of "$D/ca.crt")
  fp16=$(printf '%s' "$fp" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16)
  trust install "$D/ca.crt" --fingerprint "$fp" --yes
  ck "install exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "system file present" [ -f "/usr/local/share/ca-certificates/sylvode-ca-$fp16.crt" ]
  runlog curl -fsS -o /dev/null https://127.0.0.1/
  ck "curl without --cacert exits 0 (got $RC)" [ "$RC" = 0 ]
}

t_25() {
  trust install "$D/ca.crt" --fingerprint 00:11 --yes
  ck "exit 3 (got $RC)" [ "$RC" = 3 ]
}

t_26() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$W/bad.key" -out "$W/bad.crt" -subj /CN=unconstrained -days 5 2>/dev/null
  trust install "$W/bad.crt" --fingerprint "$(fp_of "$W/bad.crt")" --yes
  ck "exit 3 (got $RC)" [ "$RC" = 3 ]
}

t_27() {
  fp=$(fp_of "$D/ca.crt")
  fp16=$(printf '%s' "$fp" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16)
  runlog env HOME=/home/u runuser -u u -- dash /opt/tls/trust-client.sh install "$D/ca.crt" --fingerprint "$fp" --yes --target user
  ck "install --target user exits 0 (got $RC)" [ "$RC" = 0 ]
  runlog certutil -L -d sql:/home/u/.pki/nssdb
  ck "label sylvode-ca-$fp16 with trust C,," sh -c "grep 'sylvode-ca-$fp16' $LOGDIR/o | grep -q 'C,,'"
}

t_28() {
  fp=$(fp_of "$D/ca.crt")
  fp16=$(printf '%s' "$fp" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16)
  trust remove "$D/ca.crt"
  ck "remove exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "system file gone" [ ! -e "/usr/local/share/ca-certificates/sylvode-ca-$fp16.crt" ]
  runlog curl -sS -o /dev/null https://127.0.0.1/
  ck "curl without --cacert exits 60 (got $RC)" [ "$RC" = 60 ]
}

t_29() {
  nlname=$(printf 'a\nb')
  n=0
  for _spec in "--dns|a b" "--dns|*.x" "--dns|x.invalid" "--ip|010.1.1.1" "--ip|0.0.0.0" "--dns|$nlname"; do
    opt=${_spec%%|*}
    val=${_spec#*|}
    tls init-ca --dir "$W/x" "$opt" "$val"
    n=$((n + 1))
    ck "case $n exits 2 (got $RC)" [ "$RC" = 2 ]
  done
  ck "$W/x holds no CA files" sh -c "[ ! -e $W/x/ca.crt ] && [ ! -e $W/x/ca ]"
}

t_30() {
  tls init-ca --dir "$W/ipo" --ip 127.0.0.1
  ck "init-ca exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "nameConstraints contains DNS:invalid" sh -c "openssl x509 -in $W/ipo/ca.crt -noout -ext nameConstraints | grep -q 'DNS:invalid'"
  mint "$W/ipo" ipo "/O=x" "DNS:evil.example" || ck "minting the leaf" false
  runlog openssl verify -CAfile "$W/ipo/ca.crt" "$W/mint-ipo/l.crt"
  ck "hand-minted DNS leaf fails verification" [ "$RC" != 0 ]
}

o_ipv6() {
  tls init-ca --dir "$W/v6" --ip ::1
  ck "init-ca exits 0 (got $RC)" [ "$RC" = 0 ]
  tls issue --dir "$W/v6"
  ck "issue exits 0 (got $RC)" [ "$RC" = 0 ]
  serve '[::1]:8445' "$W/v6/server/server.crt" "$W/v6/server/server.key" "$W/v6/ca.crt"
  runlog curl -fsS -o /dev/null --cacert "$W/v6/ca.crt" 'https://[::1]:8445/'
  ck "curl to [::1] exits 0 (got $RC)" [ "$RC" = 0 ]
  unserve
}

run_test T-01 "shellcheck on the scripts (default and -S style)" t_01
run_test T-02 "init-ca creates a constrained CA" t_02
run_test T-03 "CA permissions" t_03
run_test T-04 "init-ca with the same names changes nothing" t_04
run_test T-05 "init-ca with other names is refused" t_05
run_test T-06 "issue with --owner; leaf profile" t_06
run_test T-07 "issue again keeps the certificate" t_07
run_test T-08 "issue --force makes a new key and serial" t_08
run_test T-09 "names outside the CA are refused" t_09
run_test T-10 "clients enforce the name constraints" t_10
run_test T-11 "the real leaf is accepted" t_11
run_test T-12 "nginx from config-hint" t_12
run_test T-13 "renew below the threshold does nothing" t_13
run_test T-14 "renew past the threshold renews and reloads" t_14
run_test T-15 "a failed hook is retried" t_15
run_test T-16 "permission self-check" t_16
run_test T-18 "path safety" t_18
run_test T-19 "--owner as a non-root user" t_19
run_test T-20 "lock and stale lock" t_20
run_test T-22 "fingerprint" t_22
run_test T-23 "config-hint content" t_23
run_test T-21 "CA rotation" t_21
run_test T-17 "no key material in any output" t_17
run_test T-24 "trust-client install (system store)" t_24
run_test T-25 "trust-client refuses a wrong fingerprint" t_25
run_test T-26 "trust-client refuses an unconstrained CA" t_26
run_test T-27 "trust-client install --target user (NSS)" t_27
run_test T-28 "trust-client remove" t_28
run_test T-29 "invalid names" t_29
run_test T-30 "IP-only CA is limited to DNS:invalid" t_30

if grep -q ' ::1$' /proc/net/if_inet6 2>/dev/null || grep -q '^00000000000000000000000000000001' /proc/net/if_inet6 2>/dev/null; then
  run_test O-IPV6 "IPv6 CA, certificate and curl" o_ipv6
else
  skip "O-IPV6 the container has no IPv6 on lo"
fi
skip "O-CADDY not run by this suite"
skip "O-BUSYBOX not run by this suite"

echo "== done: $((TOTAL - FAILS)) of $TOTAL passed; full log: $LOG"
[ "$FAILS" = 0 ]
