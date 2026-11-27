#!/bin/sh
# trust-client.sh: install, remove or check trust in a Sylvode intranet CA on macOS and Linux.
#
# The script refuses a certificate that is not a CA or that has no critical name constraint
# (made by "sylvode-tls.sh init-ca"), shows the permitted names and asks for a matching
# fingerprint or an interactive "yes" before it changes anything. Run "trust-client.sh --help".

umask 022

PROG=trust-client
OPENSSL=${OPENSSL:-openssl}
CA_FILE=
ACTION=
FINGERPRINT=
YES=0
TARGET=system
DRY_RUN=0
CHECK_URL=
LABEL=
FP=
FP16=
SHA1=
OS=
SELF_UID=$(id -u)

err() {
  printf '%s: error: %s\n' "$PROG" "$*" >&2
}

warn() {
  printf '%s: warning: %s\n' "$PROG" "$*" >&2
}

info() {
  printf '%s: %s\n' "$PROG" "$*" >&2
}

die() {
  _code=$1
  shift
  err "$*"
  exit "$_code"
}

safe_str() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9._:/@=,+ -' '?'
}

usage() {
  cat <<'EOF'
Usage: trust-client.sh install CA_FILE [--fingerprint HEX] [--yes] [--target system|user] [--dry-run]
       trust-client.sh remove  CA_FILE [--target system|user] [--dry-run]
       trust-client.sh check   CA_FILE [--target system|user] [--url URL]
       trust-client.sh --help

Install, remove or check trust in a Sylvode intranet CA certificate (made by sylvode-tls.sh
init-ca) on macOS and Linux.

install  Refuses a certificate that is not a CA or that has no critical name constraint. Prints
         the subject, validity, SHA-256 fingerprint and permitted names, then needs either a
         matching --fingerprint (colons and case are ignored) or an interactive "yes" read from
         the terminal. --yes is valid only together with --fingerprint.
remove   Removes the certificate that install added (label sylvode-ca-<16 hex digits>).
check    Reports whether the certificate is present; with --url also runs curl against the URL
         (system store). Exit 0 when present and the URL check passes, else 1.

Options:
  --fingerprint HEX  expected SHA-256 of the DER certificate, as printed by
                     "sylvode-tls.sh fingerprint" ("sha256sum" of the PEM file does not match)
  --yes              do not prompt (needs --fingerprint)
  --target system    default; needs root. macOS: System keychain. Linux: the system CA bundle.
  --target user      macOS: the login keychain (macOS asks for your password). Linux: the NSS
                     databases of Chrome, Chromium, Edge and Firefox in your home directory
                     (needs certutil, libnss3-tools on Debian); run as the desktop user, not root.
  --url URL          check: also fetch URL with curl using the system store
  --dry-run          print each command as "would run: ..." and change nothing
  --openssl PATH     OpenSSL binary used for parsing (LibreSSL is fine here)

Linux system store families: Debian/Ubuntu/Alpine (update-ca-certificates), Fedora/RHEL/CentOS/Arch
(update-ca-trust), openSUSE. Only the Debian family is covered by the acceptance tests; the others
are best effort.

Exit codes: 0 ok; 1 a command failed; 2 usage error; 3 refused (not a constrained CA, fingerprint
mismatch, not confirmed); 4 prerequisite missing (not root for system, root for Linux user, no
supported trust tool, certutil missing).
EOF
}

usage_error() {
  err "$1"
  printf 'Run trust-client.sh --help for usage.\n' >&2
  exit 2
}

# run CMD...: execute, or print in dry-run mode.
run() {
  if [ "$DRY_RUN" = 1 ]; then
    printf 'would run: %s\n' "$*"
    return 0
  fi
  "$@"
}

# say_done VERB PAST TEXT: reports a finished change, or in dry-run mode what would be done.
say_done() {
  if [ "$DRY_RUN" = 1 ]; then
    info "would $1 $3"
  else
    info "$2 $3"
  fi
}

run_or_die() {
  run "$@" || die 1 "command failed: $*"
}

# ---------------------------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------------------------

[ $# -ge 1 ] || {
  usage >&2
  exit 2
}
case $1 in
  --help | -h)
    usage
    exit 0
    ;;
  install | remove | check) ACTION=$1 ;;
  *) usage_error "unknown action: $(safe_str "$1")" ;;
esac
shift
[ $# -ge 1 ] || usage_error "CA_FILE is missing"
case $1 in
  --help | -h)
    usage
    exit 0
    ;;
  -*) usage_error "CA_FILE must come first: $(safe_str "$1")" ;;
esac
CA_FILE=$1
shift

while [ $# -gt 0 ]; do
  _a=$1
  shift
  case $_a in
    --help | -h)
      usage
      exit 0
      ;;
    --fingerprint | --target | --url | --openssl)
      [ $# -gt 0 ] || usage_error "missing value for $_a"
      _v=$1
      shift
      ;;
    --fingerprint=* | --target=* | --url=* | --openssl=*)
      _v=${_a#*=}
      _a=${_a%%=*}
      ;;
    --yes | --dry-run)
      _v=
      ;;
    *) usage_error "unknown option or argument: $(safe_str "$_a")" ;;
  esac
  case $_a in
    --fingerprint) FINGERPRINT=$_v ;;
    --target) TARGET=$_v ;;
    --url)
      [ "$ACTION" = check ] || usage_error "--url is only valid for check"
      CHECK_URL=$_v
      ;;
    --openssl) OPENSSL=$_v ;;
    --yes) YES=1 ;;
    --dry-run) DRY_RUN=1 ;;
  esac
done

case $TARGET in
  system | user) ;;
  *) usage_error "--target must be system or user" ;;
esac
if [ "$ACTION" != install ] && { [ -n "$FINGERPRINT" ] || [ "$YES" = 1 ]; }; then
  usage_error "--fingerprint and --yes are only valid for install"
fi
if [ "$YES" = 1 ] && [ -z "$FINGERPRINT" ]; then
  usage_error "--yes is only valid together with --fingerprint"
fi
if [ "$DRY_RUN" = 1 ] && [ "$ACTION" = check ]; then
  usage_error "--dry-run is not valid for check"
fi

case $(uname -s) in
  Darwin) OS=macos ;;
  Linux) OS=linux ;;
  *) die 4 "unsupported operating system: $(uname -s)" ;;
esac

# ---------------------------------------------------------------------------------------------
# Certificate checks (SD-17, SD-18)
# ---------------------------------------------------------------------------------------------

case $OPENSSL in
  */*) [ -x "$OPENSSL" ] || die 4 "OpenSSL binary not found: $(safe_str "$OPENSSL")" ;;
  *) command -v "$OPENSSL" >/dev/null 2>&1 || die 4 "openssl not found on PATH; pass --openssl PATH" ;;
esac

[ -f "$CA_FILE" ] || die 2 "not a file: $(safe_str "$CA_FILE")"
# A CA certificate from sylvode-tls.sh is under 1 KiB: cap the input before anything parses it.
_size=$(wc -c <"$CA_FILE")
[ "$((_size + 0))" -le 65536 ] || die 3 "$(safe_str "$CA_FILE") is larger than 64 KiB; refusing it"
if grep -q 'PRIVATE KEY' "$CA_FILE"; then
  die 3 "refusing $(safe_str "$CA_FILE"): it contains a private key; pass the CA certificate only"
fi
_certs=$(grep -c -e '-----BEGIN CERTIFICATE-----' "$CA_FILE")
[ "$_certs" = 1 ] || die 3 "$(safe_str "$CA_FILE") must hold exactly one PEM certificate (found $_certs)"
"$OPENSSL" x509 -in "$CA_FILE" -noout >/dev/null 2>&1 || die 3 "$(safe_str "$CA_FILE") is not a valid PEM certificate"
_der=$("$OPENSSL" x509 -in "$CA_FILE" -outform DER | wc -c)
[ "$((_der + 0))" -le 16384 ] || die 3 "the certificate in $(safe_str "$CA_FILE") is larger than 16 KiB (DER); refusing it"

FP=$("$OPENSSL" x509 -in "$CA_FILE" -noout -fingerprint -sha256 | sed -n 's/^.*Fingerprint=//p' | head -n 1 | tr 'a-f' 'A-F')
[ -n "$FP" ] || die 1 "cannot compute the SHA-256 fingerprint"
FP16=$(printf '%s' "$FP" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16)
LABEL=sylvode-ca-$FP16
# SHA-1 is only a lookup key for the macOS keychain, never a trust decision.
SHA1=$("$OPENSSL" x509 -in "$CA_FILE" -noout -fingerprint -sha1 | sed -n 's/^.*Fingerprint=//p' | head -n 1 | tr -d ':' | tr 'a-f' 'A-F')

# ---------------------------------------------------------------------------------------------
# Structural certificate parse (SD-17). The extensions are read from the DER encoding, never
# from free text, because the subject and the names can hold any characters.
# ---------------------------------------------------------------------------------------------

DER_HEX=
REST=
T=
VAL=
IS_CA=0
NC_COUNT=0
NC_CRITICAL=0
N_PERM_IP=0
N_PERM_DNS=0
PERM_LINES=
PERM_BAD=
BASE_KIND=
BASE_OK=1
EXCL_LINES=

bad_der() {
  die 3 "cannot parse the structure of $(safe_str "$CA_FILE"); refusing it"
}

# der_next: reads one TLV from the front of REST (hex). Sets T (tag), VAL (content) and REST (remainder).
der_next() {
  [ -n "$REST" ] || return 1
  T=$(printf '%s' "$REST" | cut -c1-2)
  _lb=$(printf '%s' "$REST" | cut -c3-4)
  case $T$_lb in
    *[!0-9a-f]* | ??) return 1 ;;
  esac
  _ln=$((0x$_lb))
  if [ "$_ln" -lt 128 ]; then
    _dl=$_ln
    _dh=4
  else
    _dk=$((_ln - 128))
    if [ "$_dk" -lt 1 ] || [ "$_dk" -gt 3 ]; then
      return 1
    fi
    _dh=$((4 + 2 * _dk))
    _dl=$((0x$(printf '%s' "$REST" | cut -c5-"$_dh")))
  fi
  _dt=$((_dh + 2 * _dl))
  [ "${#REST}" -ge "$_dt" ] || return 1
  if [ "$_dl" = 0 ]; then
    VAL=
  else
    VAL=$(printf '%s' "$REST" | cut -c$((_dh + 1))-"$_dt")
  fi
  REST=$(printf '%s' "$REST" | cut -c$((_dt + 1))-)
}

# der_get TAG: der_next, and the tag must be TAG.
der_get() {
  der_next || bad_der
  [ "$T" = "$1" ] || bad_der
}

hex_ascii() {
  _ha=$1
  _hout=
  while [ -n "$_ha" ]; do
    _hb=$(printf '%s' "$_ha" | cut -c1-2)
    _ha=$(printf '%s' "$_ha" | cut -c3-)
    _hn=$((0x$_hb))
    if [ "$_hn" -ge 33 ] && [ "$_hn" -le 126 ]; then
      _ho=$(printf '%o' "$_hn")
      _hout=$_hout$(printf '%b' "\\0$_ho")
    else
      _hout=$_hout'?'
    fi
  done
  printf '%s' "$_hout"
}

# dns_name_ok NAME: the DNS rules of sylvode-tls.sh (at least two labels), or the placeholder "invalid".
dns_name_ok() {
  case $1 in
    invalid) return 0 ;;
    "" | *[!a-zA-Z0-9.-]*) return 1 ;;
    .* | *. | *..* | -* | *-.* | *.-* | *-) return 1 ;;
    *[!0-9.]*) ;;
    *) return 1 ;;
  esac
  [ "${#1}" -le 253 ] || return 1
  case $1 in
    *.*) ;;
    *) return 1 ;;
  esac
  case $(printf '%s' "${1##*.}" | tr '[:upper:]' '[:lower:]') in
    invalid) return 1 ;;
  esac
  printf '%s\n' "$1" | tr '.' '\n' | while IFS= read -r _dl; do
    if [ "${#_dl}" -lt 1 ] || [ "${#_dl}" -gt 63 ]; then
      exit 1
    fi
  done
}

# describe_base: T and VAL hold a GeneralName; prints the display text, sets BASE_KIND (ip, dns,
# other) and BASE_OK (0 when the subtree is not a single host or not a valid name).
describe_base() {
  case $T in
    82)
      BASE_KIND=dns
      _bn=$(hex_ascii "$VAL")
      BASE_OK=0
      if dns_name_ok "$_bn"; then
        BASE_OK=1
      fi
      printf 'DNS:%s\n' "$_bn"
      ;;
    87)
      BASE_KIND=ip
      BASE_OK=0
      case ${#VAL} in
        16)
          case $(printf '%s' "$VAL" | cut -c9-16) in
            ffffffff) BASE_OK=1 ;;
          esac
          _ia=$(printf '%s' "$VAL" | cut -c1-8)
          _im=$(printf '%s' "$VAL" | cut -c9-16)
          printf 'IP:%s.%s.%s.%s/%s.%s.%s.%s\n' \
            $((0x$(printf '%s' "$_ia" | cut -c1-2))) $((0x$(printf '%s' "$_ia" | cut -c3-4))) \
            $((0x$(printf '%s' "$_ia" | cut -c5-6))) $((0x$(printf '%s' "$_ia" | cut -c7-8))) \
            $((0x$(printf '%s' "$_im" | cut -c1-2))) $((0x$(printf '%s' "$_im" | cut -c3-4))) \
            $((0x$(printf '%s' "$_im" | cut -c5-6))) $((0x$(printf '%s' "$_im" | cut -c7-8)))
          ;;
        64)
          case $(printf '%s' "$VAL" | cut -c33-64) in
            *[!f]*) ;;
            *) BASE_OK=1 ;;
          esac
          printf 'IP:%s/%s\n' \
            "$(printf '%s' "$VAL" | cut -c1-32 | sed 's/..../&:/g;s/:$//')" \
            "$(printf '%s' "$VAL" | cut -c33-64 | sed 's/..../&:/g;s/:$//')"
          ;;
        *) printf 'IP:(invalid length)\n' ;;
      esac
      ;;
    *)
      BASE_KIND=other
      BASE_OK=1
      printf '(other name type, tag %s)\n' "$T"
      ;;
  esac
}

parse_basic() {
  REST=$1
  der_get 30
  REST=$VAL
  if der_next && [ "$T" = 01 ] && [ "$VAL" = ff ]; then
    IS_CA=1
  fi
}

parse_nc() {
  REST=$1
  der_get 30
  REST=$VAL
  while der_next; do
    _ng=$T
    _nv=$VAL
    _no=$REST
    case $_ng in
      a0 | a1)
        REST=$_nv
        while der_next; do
          [ "$T" = 30 ] || bad_der
          _ns=$REST
          REST=$VAL
          der_next || bad_der
          _nl=$(describe_base)
          describe_base >/dev/null
          if [ "$_ng" = a0 ]; then
            if [ "$BASE_OK" != 1 ] && [ -z "$PERM_BAD" ]; then
              PERM_BAD=$_nl
            fi
            PERM_LINES=$PERM_LINES"    $_nl
"
            [ "$BASE_KIND" != ip ] || N_PERM_IP=$((N_PERM_IP + 1))
            [ "$BASE_KIND" != dns ] || N_PERM_DNS=$((N_PERM_DNS + 1))
          else
            EXCL_LINES=$EXCL_LINES"    $_nl
"
          fi
          REST=$_ns
        done
        ;;
      *) bad_der ;;
    esac
    REST=$_no
  done
}

parse_ca_der() {
  DER_HEX=$("$OPENSSL" x509 -in "$CA_FILE" -outform DER | od -An -v -tx1 | tr -d ' \n')
  [ -n "$DER_HEX" ] || bad_der
  REST=$DER_HEX
  der_get 30
  REST=$VAL
  der_get 30
  REST=$VAL
  der_next || bad_der
  if [ "$T" = a0 ]; then
    der_next || bad_der
  fi
  [ "$T" = 02 ] || bad_der
  _skip=0
  while [ "$_skip" -lt 5 ]; do
    der_get 30
    _skip=$((_skip + 1))
  done
  _ex=
  while der_next; do
    case $T in
      81 | 82) ;;
      a3)
        _ex=$VAL
        break
        ;;
      *) bad_der ;;
    esac
  done
  [ -n "$_ex" ] || bad_der
  REST=$_ex
  der_get 30
  REST=$VAL
  while der_next; do
    _xo=$REST
    REST=$VAL
    der_get 06
    _oid=$VAL
    der_next || bad_der
    _crit=0
    if [ "$T" = 01 ]; then
      [ "$VAL" != ff ] || _crit=1
      der_next || bad_der
    fi
    [ "$T" = 04 ] || bad_der
    _xv=$VAL
    case $_oid in
      551d13) parse_basic "$_xv" ;;
      551d1e)
        NC_COUNT=$((NC_COUNT + 1))
        [ "$NC_COUNT" = 1 ] || die 3 "refusing $(safe_str "$CA_FILE"): it repeats the name constraint extension"
        NC_CRITICAL=$_crit
        parse_nc "$_xv"
        ;;
    esac
    REST=$_xo
  done
}

show_certificate() {
  printf 'Certificate: %s\n' "$(safe_str "$CA_FILE")"
  printf 'Subject:     %s\n' "$("$OPENSSL" x509 -in "$CA_FILE" -noout -subject -nameopt RFC2253 | sed -n 's/^subject= *//p' | sed 's/,/, /g' | tr -c 'A-Za-z0-9._:/@=,+ -' '?')"
  printf 'Valid from:  %s\n' "$("$OPENSSL" x509 -in "$CA_FILE" -noout -startdate | sed -n 's/^notBefore=//p')"
  printf 'Valid until: %s\n' "$("$OPENSSL" x509 -in "$CA_FILE" -noout -enddate | sed -n 's/^notAfter=//p')"
  printf 'SHA-256:     %s\n' "$FP"
  printf 'This CA may vouch for these names only:\n'
  printf '  Permitted:\n%s' "$PERM_LINES"
  if [ -n "$EXCL_LINES" ]; then
    printf '  Excluded:\n%s' "$EXCL_LINES"
  fi
}

# check_constrained: SD-17 (tightened 2026-11-27): a CA with a critical name constraint that
# permits at least one IP subtree and one DNS subtree.
check_constrained() {
  parse_ca_der
  _n=$(safe_str "$CA_FILE")
  [ "$IS_CA" = 1 ] || die 3 "refusing $_n: it is not a CA certificate (basicConstraints CA:TRUE is missing)"
  [ "$NC_COUNT" = 1 ] || die 3 "refusing $_n: it has no name constraint, so it could vouch for any site"
  [ "$NC_CRITICAL" = 1 ] || die 3 "refusing $_n: its name constraint is not critical"
  if [ -n "$PERM_BAD" ]; then
    die 3 "refusing $_n: the permitted subtree $PERM_BAD is not a single host (IP masks must be all ones: /32 or /128) or not a valid DNS name (at least two labels, no leading dot, not empty)"
  fi
  if [ "$N_PERM_IP" -lt 1 ] || [ "$N_PERM_DNS" -lt 1 ]; then
    die 3 "refusing $_n: its name constraint must permit at least one IP address and one DNS name; a missing type leaves that kind of name open"
  fi
}

confirm() {
  if [ -n "$FINGERPRINT" ]; then
    _want=$(printf '%s' "$FINGERPRINT" | tr -d ':' | tr 'a-f' 'A-F')
    _have=$(printf '%s' "$FP" | tr -d ':')
    if [ "$_want" != "$_have" ]; then
      die 3 "the fingerprint does not match the certificate (expected $(safe_str "$FINGERPRINT"), computed $FP); nothing was changed"
    fi
    info "fingerprint matches"
    return 0
  fi
  if ! ( : </dev/tty ) 2>/dev/null; then
    die 3 "no terminal to confirm on; pass --fingerprint (from sylvode-tls.sh fingerprint on the server)"
  fi
  printf 'Type yes to trust this CA for the names above: ' >&2
  _ans=
  read -r _ans </dev/tty || _ans=
  [ "$_ans" = yes ] || die 3 "not confirmed; nothing was changed"
}

# ---------------------------------------------------------------------------------------------
# macOS
# ---------------------------------------------------------------------------------------------

mac_keychain() {
  if [ "$TARGET" = system ]; then
    printf '%s\n' /Library/Keychains/System.keychain
  else
    [ -n "${HOME:-}" ] || die 4 "HOME is not set"
    printf '%s\n' "$HOME/Library/Keychains/login.keychain-db"
  fi
}

mac_need_root() {
  if [ "$TARGET" = system ] && [ "$SELF_UID" != 0 ] && [ "$DRY_RUN" != 1 ]; then
    die 4 "the system keychain needs root: run with sudo, or use --target user"
  fi
  command -v security >/dev/null 2>&1 || die 4 "the security tool is missing"
}

mac_install() {
  mac_need_root
  _kc=$(mac_keychain)
  if [ "$TARGET" = system ]; then
    run_or_die security add-trusted-cert -d -r trustRoot -k "$_kc" "$CA_FILE"
  else
    run_or_die security add-trusted-cert -r trustRoot -k "$_kc" "$CA_FILE"
  fi
  say_done install installed "$LABEL in $_kc"
  printf 'Hints:\n'
  printf '  Firefox: set security.enterprise_roots.enabled to true in about:config if it is not already (the default in current releases).\n'
  printf '  Node.js: NODE_EXTRA_CA_CERTS=%s (read at start-up), or NODE_USE_SYSTEM_CA=1 on recent releases.\n' "$CA_FILE"
  printf '  Python requests/httpx: REQUESTS_CA_BUNDLE or SSL_CERT_FILE pointing at %s.\n' "$CA_FILE"
}

mac_remove() {
  mac_need_root
  _kc=$(mac_keychain)
  if [ "$TARGET" = system ]; then
    run_or_die security remove-trusted-cert -d "$CA_FILE"
  else
    run_or_die security remove-trusted-cert "$CA_FILE"
  fi
  run_or_die security delete-certificate -Z "$SHA1" "$_kc"
  say_done remove removed "$LABEL from $_kc"
}

mac_present() {
  _kc=$(mac_keychain)
  security find-certificate -a -Z "$_kc" 2>/dev/null | grep -qi "SHA-1 hash: $SHA1"
}

# ---------------------------------------------------------------------------------------------
# Linux
# ---------------------------------------------------------------------------------------------

FAMILY=
DEST=

detect_linux() {
  if command -v update-ca-certificates >/dev/null 2>&1 && [ -d /usr/local/share/ca-certificates ]; then
    FAMILY=debian
    DEST=/usr/local/share/ca-certificates/$LABEL.crt
  elif command -v update-ca-trust >/dev/null 2>&1; then
    FAMILY=redhat
    if [ -d /etc/ca-certificates/trust-source/anchors ]; then
      DEST=/etc/ca-certificates/trust-source/anchors/$LABEL.pem
    else
      DEST=/etc/pki/ca-trust/source/anchors/$LABEL.pem
    fi
  elif [ -d /etc/pki/trust/anchors ] && command -v update-ca-certificates >/dev/null 2>&1; then
    FAMILY=suse
    DEST=/etc/pki/trust/anchors/$LABEL.pem
  else
    die 4 "no supported system trust tool found (update-ca-certificates or update-ca-trust)"
  fi
}

linux_system_need_root() {
  if [ "$SELF_UID" != 0 ] && [ "$DRY_RUN" != 1 ]; then
    die 4 "the system store needs root: run with sudo, or use --target user"
  fi
}

# nss_dirs CREATE: prints each NSS database directory (one per line).
nss_dirs() {
  [ -n "${HOME:-}" ] || die 4 "HOME is not set"
  if [ "$1" = 1 ] && [ ! -d "$HOME/.pki/nssdb" ]; then
    run_or_die mkdir -p "$HOME/.pki/nssdb"
    run_or_die certutil -N --empty-password -d "sql:$HOME/.pki/nssdb"
  fi
  if [ -d "$HOME/.pki/nssdb" ]; then
    printf '%s\n' "$HOME/.pki/nssdb"
  elif [ "$DRY_RUN" = 1 ] && [ "$1" = 1 ]; then
    printf '%s\n' "$HOME/.pki/nssdb"
  fi
  for _root in "$HOME/.mozilla/firefox" "$HOME/snap/firefox/common/.mozilla/firefox" "$HOME/.var/app/org.mozilla.firefox/.mozilla/firefox"; do
    [ -d "$_root" ] || continue
    find "$_root" -maxdepth 2 -name cert9.db 2>/dev/null | while IFS= read -r _f; do
      printf '%s\n' "${_f%/*}"
    done
  done
}

linux_user_prereq() {
  [ "$SELF_UID" != 0 ] || die 4 "--target user must not run as root: run as the desktop user"
  command -v certutil >/dev/null 2>&1 || die 4 "certutil is missing (Debian: apt install libnss3-tools)"
}

linux_install() {
  if [ "$TARGET" = system ]; then
    linux_system_need_root
    detect_linux
    run_or_die "$OPENSSL" x509 -in "$CA_FILE" -out "$DEST"
    run_or_die chmod 0644 "$DEST"
    case $FAMILY in
      debian | suse) run_or_die update-ca-certificates ;;
      redhat) run_or_die update-ca-trust extract ;;
    esac
    say_done install installed "$LABEL as $DEST"
    HINT_CA=$DEST
  else
    linux_user_prereq
    warn "close Firefox before installing; it may not pick the change up while running"
    _dirs=$(nss_dirs 1)
    printf '%s\n' "$_dirs" | while IFS= read -r _d; do
      [ -n "$_d" ] || continue
      run_or_die certutil -A -d "sql:$_d" -n "$LABEL" -t "C,," -i "$CA_FILE"
      say_done install installed "$LABEL in $_d"
    done
    HINT_CA=$CA_FILE
  fi
  printf 'Hints:\n'
  if [ "$TARGET" = system ]; then
    printf '  Chrome, Chromium and Firefox on Linux use their own NSS databases: also run "trust-client.sh install %s --target user" as the desktop user.\n' "$CA_FILE"
  fi
  printf '  Firefox: if it does not use the NSS database, set security.enterprise_roots.enabled to true in about:config.\n'
  printf '  Node.js: NODE_EXTRA_CA_CERTS=%s (read at start-up), or NODE_USE_SYSTEM_CA=1 on recent releases.\n' "$HINT_CA"
  printf '  Python requests/httpx: REQUESTS_CA_BUNDLE or SSL_CERT_FILE pointing at the system bundle or %s.\n' "$HINT_CA"
}

linux_remove() {
  if [ "$TARGET" = system ]; then
    linux_system_need_root
    detect_linux
    run_or_die rm -f "$DEST"
    case $FAMILY in
      debian) run_or_die update-ca-certificates --fresh ;;
      suse) run_or_die update-ca-certificates ;;
      redhat) run_or_die update-ca-trust extract ;;
    esac
    say_done remove removed "$LABEL ($DEST)"
  else
    linux_user_prereq
    warn "close Firefox before removing; it may not pick the change up while running"
    _dirs=$(nss_dirs 0)
    printf '%s\n' "$_dirs" | while IFS= read -r _d; do
      [ -n "$_d" ] || continue
      if certutil -L -d "sql:$_d" -n "$LABEL" >/dev/null 2>&1; then
        run_or_die certutil -D -d "sql:$_d" -n "$LABEL"
        say_done remove removed "$LABEL from $_d"
      fi
    done
  fi
}

# linux_present: prints the stores checked; returns 0 when present in all checked target stores.
linux_present() {
  if [ "$TARGET" = system ]; then
    detect_linux
    if [ -f "$DEST" ]; then
      printf 'present: %s\n' "$DEST"
      return 0
    fi
    printf 'missing: %s\n' "$DEST"
    return 1
  fi
  linux_user_prereq
  _found=0
  _missing=0
  _dirs=$(nss_dirs 0)
  while IFS= read -r _d; do
    [ -n "$_d" ] || continue
    if certutil -L -d "sql:$_d" -n "$LABEL" >/dev/null 2>&1; then
      printf 'present: %s (%s)\n' "$LABEL" "$_d"
      _found=1
    else
      printf 'missing: %s (%s)\n' "$LABEL" "$_d"
      _missing=1
    fi
  done <<EOF2
$_dirs
EOF2
  [ "$_found" = 1 ] && [ "$_missing" = 0 ]
}

# ---------------------------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------------------------

HINT_CA=$CA_FILE

case $ACTION in
  install)
    check_constrained
    show_certificate
    confirm
    if [ "$OS" = macos ]; then mac_install; else linux_install; fi
    ;;
  remove)
    if [ "$OS" = macos ]; then mac_remove; else linux_remove; fi
    ;;
  check)
    _rc=0
    if [ "$OS" = macos ]; then
      command -v security >/dev/null 2>&1 || die 4 "the security tool is missing"
      if mac_present; then
        printf 'present: %s in %s\n' "$LABEL" "$(mac_keychain)"
      else
        printf 'missing: %s in %s\n' "$LABEL" "$(mac_keychain)"
        _rc=1
      fi
    else
      linux_present || _rc=1
    fi
    if [ -n "$CHECK_URL" ]; then
      command -v curl >/dev/null 2>&1 || die 4 "curl is missing"
      curl -fsS -o /dev/null "$CHECK_URL"
      _crc=$?
      if [ "$_crc" = 0 ]; then
        printf 'url check passed: %s\n' "$CHECK_URL"
      else
        printf 'url check failed: %s (curl exit %s)\n' "$CHECK_URL" "$_crc"
        _rc=1
      fi
    fi
    exit "$_rc"
    ;;
esac
