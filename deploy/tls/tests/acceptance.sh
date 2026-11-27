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
cp "$SRC/sylvode-tls.sh" "$SRC/trust-client.sh" "$SRC/trust-client.ps1" /opt/tls/
chmod 0755 /opt/tls/sylvode-tls.sh /opt/tls/trust-client.sh
openssl version

# PowerShell 7 (pinned release and checksum) to run trust-client.ps1 on Linux .NET.
PWSH_OK=0
case $(uname -m) in
  aarch64 | arm64)
    PWSH_ARCH=arm64
    PWSH_SHA=c0159b03e85f44ae1e7697818a011558da6c813d0aae848bf5ac13bf435d8624
    ;;
  x86_64)
    PWSH_ARCH=x64
    PWSH_SHA=6f6015203c47806c5cc444c19d8ed019695e610fbd948154264bf9ca8e157561
    ;;
  *) PWSH_ARCH= ;;
esac
if [ -n "$PWSH_ARCH" ]; then
  if curl -fsSL -o "$LOGDIR/pwsh.tgz" "https://github.com/PowerShell/PowerShell/releases/download/v7.4.6/powershell-7.4.6-linux-$PWSH_ARCH.tar.gz" &&
    [ "$(sha256sum "$LOGDIR/pwsh.tgz" | cut -d' ' -f1)" = "$PWSH_SHA" ] &&
    mkdir -p /opt/pwsh && tar -xzf "$LOGDIR/pwsh.tgz" -C /opt/pwsh && chmod 0755 /opt/pwsh/pwsh; then
    PWSH_VER=$(DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1 /opt/pwsh/pwsh -NoProfile -Command "\$PSVersionTable.PSVersion.ToString()" 2>/dev/null)
    if [ -n "$PWSH_VER" ]; then
      PWSH_OK=1
      echo "PowerShell $PWSH_VER"
    fi
  fi
fi
echo "== running"

# hostile NAME BASICCONSTRAINTS EXTENSION-LINE: a self-signed CA whose subject imitates the text
# a naive parser looks for.
hostile() {
  printf '[req]\ndistinguished_name = dn\nprompt = no\nx509_extensions = x\n[dn]\nO = X509v3 Name Constraints: critical\nCN = CA:TRUE\n[x]\nbasicConstraints = %s\n%s\n' "$2" "$3" >"$W/h-$1.cnf"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$W/h-$1.key" -out "$W/h-$1.crt" -days 5 -config "$W/h-$1.cnf" 2>/dev/null
}

# hostile_inject: DNS-only constraint whose DNS name carries a newline and a fake IP line.
hostile_inject() {
  _s=$(printf 'a\n      IP:10.0.0.1/255.255.255.255' | od -An -v -tx1 | tr -d ' \n')
  _sl=$(printf '%02x' $(( ${#_s} / 2 )))
  _e=82$_sl$_s
  _el=$(printf '%02x' $(( ${#_e} / 2 )))
  _t=30$_el$_e
  _tl=$(printf '%02x' $(( ${#_t} / 2 )))
  _g=a0$_tl$_t
  _gl=$(printf '%02x' $(( ${#_g} / 2 )))
  hostile inject "critical,CA:TRUE" "nameConstraints = critical,DER:30$_gl$_g"
}

# der_wrap TAG HEX: a TLV (lengths up to 255).
der_wrap() {
  _w=$2
  _wl=$((${#_w} / 2))
  if [ "$_wl" -lt 128 ]; then
    printf '%s%02x%s' "$1" "$_wl" "$_w"
  else
    printf '%s81%02x%s' "$1" "$_wl" "$_w"
  fi
}

ascii_hex() {
  printf '%s' "$1" | od -An -v -tx1 | tr -d ' \n'
}

ip_subtree() {
  der_wrap 30 "$(der_wrap 87 "$1$2")"
}

dns_subtree() {
  der_wrap 30 "$(der_wrap 82 "$(ascii_hex "$1")")"
}

# hostile_der NAME PERMITTED-SUBTREES-HEX: a CA whose name constraint is built by hand.
hostile_der() {
  hostile "$1" "critical,CA:TRUE" "nameConstraints = critical,DER:$(der_wrap 30 "$(der_wrap a0 "$2")")"
}

# hostile_big: a CA with a good constraint and a certificate larger than 16 KiB.
hostile_big() {
  {
    printf '[req]\ndistinguished_name = dn\nprompt = no\nx509_extensions = x\n[dn]\nO = x\nCN = big\n[x]\n'
    printf 'basicConstraints = critical,CA:TRUE\n'
    printf 'nameConstraints = critical,permitted;DNS:a.test,permitted;IP:10.0.0.1/255.255.255.255\n'
    printf 'subjectAltName = @san\n[san]\n'
    _i=0
    while [ "$_i" -lt 1500 ]; do
      printf 'DNS.%s = a%s.example\n' "$_i" "$_i"
      _i=$((_i + 1))
    done
  } >"$W/h-big.cnf"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$W/h-big.key" -out "$W/h-big.crt" -days 5 -config "$W/h-big.cnf" 2>/dev/null
}

# make_m5_cas: the hostile CAs of the second re-check, F13 and F14.
make_m5_cas() {
  hostile_der ipall "$(ip_subtree 00000000 00000000)$(dns_subtree a.test)"
  hostile_der emptydns "$(ip_subtree 0a000001 ffffffff)$(der_wrap 30 "$(der_wrap 82 '')")"
  hostile ip8 "critical,CA:TRUE" "nameConstraints = critical,permitted;IP:10.0.0.0/255.0.0.0,permitted;DNS:example.com"
  hostile ip6w "critical,CA:TRUE" "nameConstraints = critical,permitted;IP:fd00::/ffff:ffff:ffff:ffff:0:0:0:0,permitted;DNS:example.com"
  hostile dotdns "critical,CA:TRUE" "nameConstraints = critical,permitted;DNS:.example.com,permitted;IP:10.0.0.1/255.255.255.255"
  hostile single "critical,CA:TRUE" "nameConstraints = critical,permitted;DNS:com,permitted;IP:10.0.0.1/255.255.255.255"
  hostile_big
}

# trust_refused NAME: trust-client.sh must refuse the hostile CA with exit 3 and install nothing.
trust_refused() {
  _fp=$(fp_of "$W/h-$1.crt")
  _f16=$(printf '%s' "$_fp" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16)
  trust install "$W/h-$1.crt" --fingerprint "$_fp" --yes
  ck "$1: exit 3 (got $RC)" [ "$RC" = 3 ]
  ck "$1: nothing installed" [ ! -e "/usr/local/share/ca-certificates/sylvode-ca-$_f16.crt" ]
}

ps() {
  runlog env HOME=/root DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1 /opt/pwsh/pwsh -NoProfile -File /opt/tls/trust-client.ps1 "$@"
}

# ps_refused NAME: trust-client.ps1 must refuse the hostile CA with exit 3.
ps_refused() {
  ps -Action Install -CertPath "$W/h-$1.crt" -Fingerprint "$(fp_of "$W/h-$1.crt")" -Yes -Scope CurrentUser
  ck "$1: exit 3 (got $RC)" [ "$RC" = 3 ]
}

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


t_26b() {
  # M1: subject text that imitates the extension names; no real name constraint.
  hostile spoof "critical,CA:TRUE" ""
  trust_refused spoof
  # M1: a CA:FALSE certificate with a good constraint and a spoofed subject.
  hostile cafalse "critical,CA:FALSE" "nameConstraints = critical,permitted;DNS:a.test,permitted;IP:10.0.0.1/255.255.255.255"
  trust_refused cafalse
}

t_26c() {
  # M2: constraints that leave a name type open.
  hostile dnsonly "critical,CA:TRUE" "nameConstraints = critical,permitted;DNS:sylvode.lan"
  trust_refused dnsonly
  hostile iponly "critical,CA:TRUE" "nameConstraints = critical,permitted;IP:10.0.0.1/255.255.255.255"
  trust_refused iponly
  hostile excl "critical,CA:TRUE" "nameConstraints = critical,excluded;DNS:nothing.test"
  trust_refused excl
  hostile noncrit "critical,CA:TRUE" "nameConstraints = permitted;DNS:a.test,permitted;IP:10.0.0.1/255.255.255.255"
  trust_refused noncrit
}

t_26d() {
  # M1/M2: a DNS-only constraint whose name carries a newline and a fake IP line.
  hostile_inject
  ck "the injected certificate was created" [ -f "$W/h-inject.crt" ]
  trust_refused inject
  # control: a well-formed hand-made CA with both subtrees is accepted (dry run)
  hostile good "critical,CA:TRUE" "nameConstraints = critical,permitted;DNS:a.test,permitted;IP:10.0.0.1/255.255.255.255"
  trust install "$W/h-good.crt" --fingerprint "$(fp_of "$W/h-good.crt")" --yes --dry-run
  ck "control: both subtrees accepted (got $RC)" [ "$RC" = 0 ]
}

t_26e() {
  # M5: permitted subtrees that match everything or a whole range; F13: a single-label DNS base;
  # F14: an oversized certificate.
  make_m5_cas
  for _n in ipall emptydns ip8 ip6w dotdns single big; do
    ck "$_n was created" [ -f "$W/h-$_n.crt" ]
    trust_refused "$_n"
  done
  ck "oversized certificate is refused for its size" err_has '16 KiB'
  runlog dash /opt/tls/sylvode-tls.sh init-ca --dir "$W/x2" --dns localhost
  ck "init-ca refuses a single-label name with exit 2 (got $RC)" [ "$RC" = 2 ]
}

t_31() {
  # M4: the cron and systemd command lines of docs/install.md, verbatim.
  cron=$(grep -E '^17 3 \* \* \* root ' /src/docs/install.md | head -n 1)
  unit=$(grep -E '^ExecStart=/usr/local/sbin/sylvode-tls.sh' /src/docs/install.md | head -n 1)
  ck "cron line found in the docs" [ -n "$cron" ]
  ck "ExecStart line found in the docs" [ -n "$unit" ]
  cmd=${cron#"17 3 * * * root "}
  execline=${unit#ExecStart=}
  install -m 0755 /opt/tls/sylvode-tls.sh /usr/local/sbin/sylvode-tls.sh
  # A stand-in for systemctl: the container has no systemd. It accepts only "reload nginx".
  printf '%s\n' '#!/bin/sh' "[ \"\$1 \$2\" = \"reload nginx\" ] || exit 1" 'touch /tmp/systemctl-reload' >/usr/local/bin/systemctl
  chmod 0755 /usr/local/bin/systemctl
  runlog /usr/local/sbin/sylvode-tls.sh init-ca --ip 127.0.0.1
  ck "init-ca in the default directory (got $RC)" [ "$RC" = 0 ]
  runlog /usr/local/sbin/sylvode-tls.sh issue --days 30
  ck "issue in the default directory (got $RC)" [ "$RC" = 0 ]
  s0=$(serial_of /var/lib/sylvode-tls/server/server.crt)
  rm -f /tmp/systemctl-reload /var/log/sylvode-tls.log
  runlog sh -c "$cmd"
  ck "cron line exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "cron line renewed the certificate" [ "$(serial_of /var/lib/sylvode-tls/server/server.crt)" != "$s0" ]
  ck "cron line ran the reload hook" [ -e /tmp/systemctl-reload ]
  ck "cron log holds no usage or error text" sh -c "! grep -qiE 'usage|error|unknown' /var/log/sylvode-tls.log"
  s1=$(serial_of /var/lib/sylvode-tls/server/server.crt)
  rm -f /tmp/systemctl-reload
  runlog sh -c "$execline"
  ck "ExecStart line exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "ExecStart line renewed the certificate" [ "$(serial_of /var/lib/sylvode-tls/server/server.crt)" != "$s1" ]
  ck "ExecStart line ran the reload hook" [ -e /tmp/systemctl-reload ]
}

t_32() {
  # F1: a symlink planted as .reload-pending is refused, its target is not truncated.
  tls init-ca --dir "$W/f1" --ip 127.0.0.1
  tls issue --dir "$W/f1"
  echo precious >"$W/victim"
  ln -s "$W/victim" "$W/f1/server/.reload-pending"
  tls renew --dir "$W/f1" --force --exec true
  ck "exit 3 (got $RC)" [ "$RC" = 3 ]
  ck "the symlink target is intact" [ "$(cat "$W/victim")" = precious ]
}

t_33() {
  # F2: an interrupted promote (new key already installed) is not destroyed by --abort.
  tls init-ca --dir "$W/f2" --ip 127.0.0.1
  tls issue --dir "$W/f2"
  tls rotate-ca --dir "$W/f2"
  ck "rotate-ca exits 0 (got $RC)" [ "$RC" = 0 ]
  nh=$(h "$W/f2/next.crt")
  mv -f "$W/f2/next/ca.key" "$W/f2/ca/ca.key"
  tls rotate-ca --dir "$W/f2" --abort
  ck "--abort exits 3 (got $RC)" [ "$RC" = 3 ]
  ck "next.crt is kept" [ -f "$W/f2/next.crt" ]
  tls rotate-ca --dir "$W/f2" --promote
  ck "--promote resumes and exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "ca.crt is the former next.crt" [ "$(h "$W/f2/ca.crt")" = "$nh" ]
  tls issue --dir "$W/f2" --force
  ck "issue works afterwards (got $RC)" [ "$RC" = 0 ]
}

t_34() {
  # F9: a dry run says "would install", not "installed".
  fp=$(fp_of "$D/ca.crt")
  trust install "$D/ca.crt" --fingerprint "$fp" --yes --dry-run
  ck "dry run exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "says would install" err_has 'would install'
  ck "does not say installed" sh -c "! grep -q ' installed ' $LOGDIR/e"
}

t_35() {
  # F7: the script and its children run with a zero core file limit.
  rm -f /tmp/ulim
  tls init-ca --dir "$W/f35" --ip 127.0.0.1
  runlog sh -c "ulimit -c unlimited 2>/dev/null; dash /opt/tls/sylvode-tls.sh issue --dir $W/f35 --force --exec 'ulimit -c >/tmp/ulim'"
  ck "issue exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "core limit is 0 (got $(cat /tmp/ulim 2>/dev/null))" [ "$(cat /tmp/ulim 2>/dev/null)" = 0 ]
}

p_01() {
  ps -Action Install -CertPath "$D/ca.crt" -Fingerprint "$(fp_of "$D/ca.crt")" -Yes -Scope CurrentUser -DryRun
  ck "DryRun install exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "prints would run" out_has 'would run'
  ps -Action Install -CertPath "$D/ca.crt" -Fingerprint 00:11 -Yes -Scope CurrentUser
  ck "wrong fingerprint exits 3 (got $RC)" [ "$RC" = 3 ]
  ps -Action Install -CertPath "$D/ca.crt" -Yes -Scope CurrentUser
  ck "-Yes without -Fingerprint exits 2 (got $RC)" [ "$RC" = 2 ]
}

p_02() {
  fp=$(fp_of "$D/ca.crt")
  ps -Action Check -CertPath "$D/ca.crt" -Scope CurrentUser
  ck "Check before install exits 1 (got $RC)" [ "$RC" = 1 ]
  ps -Action Install -CertPath "$D/ca.crt" -Fingerprint "$fp" -Yes -Scope CurrentUser
  ck "Install exits 0 (got $RC)" [ "$RC" = 0 ]
  ps -Action Check -CertPath "$D/ca.crt" -Scope CurrentUser
  ck "Check after install exits 0 (got $RC)" [ "$RC" = 0 ]
  ck "Check says present" out_has 'present'
  ps -Action Remove -CertPath "$D/ca.crt" -Scope CurrentUser
  ck "Remove exits 0 (got $RC)" [ "$RC" = 0 ]
  ps -Action Check -CertPath "$D/ca.crt" -Scope CurrentUser
  ck "Check after Remove exits 1 (got $RC)" [ "$RC" = 1 ]
  ck "Check says missing" out_has 'missing'
}

p_03() {
  for _n in spoof cafalse dnsonly iponly excl noncrit inject ipall emptydns ip8 ip6w dotdns single big; do
    ps_refused "$_n"
  done
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
run_test T-26b "trust-client refuses a spoofed subject and a CA:FALSE certificate (M1)" t_26b
run_test T-26c "trust-client refuses constraints that leave a name type open (M2)" t_26c
run_test T-26d "trust-client refuses a newline-injected DNS-only constraint" t_26d
run_test T-26e "trust-client refuses match-all, wide, single-label and oversized CAs (M5, F13, F14)" t_26e
run_test T-31 "the cron and systemd lines of docs/install.md work (M4)" t_31
run_test T-32 ".reload-pending symlink is refused (F1)" t_32
run_test T-33 "an interrupted promote survives --abort (F2)" t_33
run_test T-34 "dry run says would install (F9)" t_34
run_test T-35 "core files are disabled (F7)" t_35
if [ "$PWSH_OK" = 1 ]; then
  run_test P-01 "trust-client.ps1 DryRun, wrong fingerprint, -Yes alone" p_01
  run_test P-02 "trust-client.ps1 Install, Check, Remove (M3)" p_02
  run_test P-03 "trust-client.ps1 refuses the hostile CAs (M1, M2)" p_03
else
  skip "P-01..P-03 PowerShell 7 could not be installed"
fi

if grep -q ' ::1$' /proc/net/if_inet6 2>/dev/null || grep -q '^00000000000000000000000000000001' /proc/net/if_inet6 2>/dev/null; then
  run_test O-IPV6 "IPv6 CA, certificate and curl" o_ipv6
else
  skip "O-IPV6 the container has no IPv6 on lo"
fi
skip "O-CADDY not run by this suite"
skip "O-BUSYBOX not run by this suite"

echo "== done: $((TOTAL - FAILS)) of $TOTAL passed; full log: $LOG"
[ "$FAILS" = 0 ]
