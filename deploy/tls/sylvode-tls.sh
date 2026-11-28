#!/bin/sh
# sylvode-tls.sh: a private, name-constrained CA and a server certificate for Sylvode on a
# private network (WireGuard, LAN) without public DNS or ACME.
#
# Requires POSIX sh and OpenSSL 3.0 or newer. Run "sylvode-tls.sh --help" for the subcommands and
# "sylvode-tls.sh <subcommand> --help" for the options of one subcommand. Usage is documented in
# docs/install.md ("HTTPS on a private network").
#
# The CA private key never leaves DIR/ca (DIR/next during a rotation). Nothing this script prints
# contains key material.

umask 077
# No core file: a crash while openssl holds the CA key must not leave key material on disk.
# shellcheck disable=SC3045 # ulimit -c is not in POSIX but dash, bash and macOS sh all have it
ulimit -c 0 2>/dev/null

PROG=sylvode-tls
VERSION=1
NL='
'

QUIET=0
LOCK_HELD=0
CLEAN_DIRS=
DIR=
SUB=
NAMES=
NOWARN=0
OPENSSL=
SSL_OK=0
SSL_LIBRE=0
SSL_VERSION=
TMPD=
KEY_UNREADABLE=0
SELF_UID=$(id -u)

# ---------------------------------------------------------------------------------------------
# Messages, cleanup, helpers
# ---------------------------------------------------------------------------------------------

info() {
  if [ "$QUIET" != 1 ]; then
    printf '%s: %s\n' "$PROG" "$*" >&2
  fi
  return 0
}

warn() {
  printf '%s: warning: %s\n' "$PROG" "$*" >&2
}

warnn() {
  if [ "$NOWARN" != 1 ]; then
    warn "$@"
  fi
  return 0
}

err() {
  printf '%s: error: %s\n' "$PROG" "$*" >&2
}

die() {
  _die_code=$1
  shift
  err "$*"
  exit "$_die_code"
}

# Printable form of an untrusted string (control characters and odd bytes become "?").
safe_str() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9._:/@=,+ -' '?'
}

release_lock() {
  if [ "$LOCK_HELD" = 1 ]; then
    rm -rf "${DIR:?}/.lock"
    LOCK_HELD=0
  fi
  return 0
}

cleanup() {
  _cl_rc=$?
  trap - EXIT
  if [ -n "$CLEAN_DIRS" ]; then
    printf '%s' "$CLEAN_DIRS" | while IFS= read -r _cl_d; do
      if [ -n "$_cl_d" ]; then
        rm -rf "$_cl_d"
      fi
    done
  fi
  release_lock
  exit "$_cl_rc"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# mk_tmp PARENT: creates a 0700 temporary directory inside PARENT; sets TMPD.
mk_tmp() {
  TMPD=$(mktemp -d "$1/.tmp.XXXXXX") || die 1 "cannot create a temporary directory in $1"
  CLEAN_DIRS="${CLEAN_DIRS}${TMPD}${NL}"
  chmod 0700 "$TMPD" || die 1 "cannot set the mode of $TMPD"
}

# is_int VALUE MIN MAX
is_int() {
  case $1 in
    "" | *[!0-9]*) return 1 ;;
  esac
  [ "${#1}" -le 9 ] || return 1
  [ "$1" -ge "$2" ] && [ "$1" -le "$3" ]
}

# find_match PATH FIND-ARGS...: true when find prints something.
find_match() {
  [ -n "$(find "$@" 2>/dev/null)" ]
}

# ---------------------------------------------------------------------------------------------
# Usage and help
# ---------------------------------------------------------------------------------------------

usage_main() {
  cat <<'EOF'
Usage: sylvode-tls.sh <subcommand> [options]
       sylvode-tls.sh --help | --version

A private, name-constrained CA and a server certificate for Sylvode on a private network.

Subcommands:
  init-ca       create the CA (limited to the given IP addresses and DNS names)
  issue         issue the server certificate
  renew         renew the server certificate when it is due
  rotate-ca     create, promote or abort a replacement CA
  fingerprint   print the SHA-256 fingerprint of the CA certificate
  status        show the CA and the server certificate
  config-hint   print Sylvode and reverse proxy settings (nginx, Caddy)

Common options:
  --dir DIR       state directory (default: $SYLVODE_TLS_DIR, else /var/lib/sylvode-tls as root,
                  else ${XDG_DATA_HOME:-$HOME/.local/share}/sylvode-tls)
  --openssl PATH  OpenSSL 3 binary (default: $OPENSSL, else openssl on PATH). LibreSSL is refused
                  except for fingerprint and status. On macOS use Homebrew openssl@3.
  --quiet         suppress informational messages (errors and warnings stay)
  --help, -h      show this help; "<subcommand> --help" shows the options of a subcommand

Exit codes: 0 ok; 1 unexpected failure; 2 usage error or invalid name; 3 refused by policy;
4 prerequisite missing; 5 reload hook failed; 6 another instance holds the lock;
10 "renew --check": renewal is due.

Documentation: docs/install.md, section "HTTPS on a private network".
EOF
}

usage_line() {
  case $1 in
    init-ca) printf '%s\n' 'sylvode-tls.sh init-ca (--ip ADDR | --dns NAME)... [--ca-days N] [--ca-name TEXT] [--dir DIR]' ;;
    issue) printf '%s\n' 'sylvode-tls.sh issue [--ip ADDR]... [--dns NAME]... [--out OUT] [--owner USER[:GROUP]] [--days N] [--force] [--exec CMD] [--dir DIR]' ;;
    renew) printf '%s\n' 'sylvode-tls.sh renew [--out OUT] [--threshold-days N] [--days N] [--owner USER[:GROUP]] [--force] [--check] [--exec CMD] [--dir DIR]' ;;
    rotate-ca) printf '%s\n' 'sylvode-tls.sh rotate-ca [(--ip ADDR | --dns NAME)...] [--ca-days N] [--ca-name TEXT] [--dir DIR] | --promote | --abort' ;;
    fingerprint) printf '%s\n' 'sylvode-tls.sh fingerprint [--cert FILE] [--dir DIR]' ;;
    status) printf '%s\n' 'sylvode-tls.sh status [--out OUT] [--threshold-days N] [--dir DIR]' ;;
    config-hint) printf '%s\n' 'sylvode-tls.sh config-hint [--out OUT] [--name NAME] [--port N] [--http-port N] [--upstream HOST:PORT] [--proxy nginx|caddy|all] [--cert-path PATH] [--key-path PATH] [--dir DIR]' ;;
    *) printf '%s\n' 'sylvode-tls.sh <subcommand> [options]' ;;
  esac
}

help_sub() {
  printf 'Usage: %s\n\n' "$(usage_line "$SUB")"
  case $SUB in
    init-ca)
      cat <<'EOF'
Create the CA. Its certificate carries a critical name constraint: it can vouch for the listed
names only (a DNS name also covers its subdomains). Without a DNS name the CA is limited to the
placeholder "invalid"; without an IP address, to 255.255.255.255.

  --ip ADDR       permitted IP address (repeatable); IPv6 in RFC 5952 form
  --dns NAME      permitted DNS name (repeatable); no wildcards, lowercase letters, digits, "-"
  --ca-days N     CA lifetime in days, 30-7300 (default 3650)
  --ca-name TEXT  CA common name, [A-Za-z0-9 ._-], 1-64 characters
                  (default "Sylvode intranet CA <date> <8 hex>")

The CA key is written to DIR/ca/ca.key (0400, unencrypted) and never leaves that directory.
Repeating the command with the same names changes nothing; other names need rotate-ca.
EOF
      ;;
    issue)
      cat <<'EOF'
Issue the server certificate (new EC P-256 key every time) into OUT.

  --ip ADDR, --dns NAME  names for the certificate (default: all names of the CA); each must be
                         one of the CA's permitted names
  --out OUT              output directory (default DIR/server); it and every parent must be owned
                         by root or the invoking user and not group- or world-writable
  --owner USER[:GROUP]   owner of server.key (root only); other files stay with the invoking user
  --days N               lifetime in days, 1-825 (default 397; capped by the CA's expiry)
  --force                reissue even when a matching valid certificate exists
  --exec CMD             run "sh -c CMD" after a new certificate was written; it receives
                         SYLVODE_TLS_OUT, _CERT, _KEY, _FULLCHAIN and _NOT_AFTER (paths only)

Files: OUT/server.key (0600), server.crt, fullchain.pem (leaf, then CA), issue.conf.
EOF
      ;;
    renew)
      cat <<'EOF'
Renew the server certificate in OUT when it is due: it expires within the threshold, it is not
signed by the current CA, its key does not match, or one of its names left the CA's list.

  --out OUT               directory with server.crt, server.key, issue.conf (default DIR/server)
  --threshold-days N      renew when fewer than N days remain, 1-400 (default 60). With an
                          explicit --days the threshold must be lower than it (else exit 2). A
                          threshold not lower than the stored lifetime only warns: every run then
                          renews, because each new certificate is due at once
  --days N                lifetime of the new certificate (default: days= of issue.conf)
  --owner USER[:GROUP]    owner of server.key (default: owner= of issue.conf)
  --force                 renew now
  --check                 only evaluate: exit 10 when due, 0 when not; writes nothing, runs no hook
  --exec CMD              run "sh -c CMD" after a new certificate was written, or when an earlier
                          hook failed (OUT/.reload-pending); see "issue --help" for its environment

Exit code 5 means the certificate was written but the hook failed.
EOF
      ;;
    rotate-ca)
      cat <<'EOF'
Replace the CA: for expiry, for a changed list of names, or after a suspected CA key leak.

  rotate-ca [names] [--ca-days N] [--ca-name TEXT]
                  create a pending CA in DIR/next* (names default to the current ones); install
                  DIR/next.crt on every client afterwards
  rotate-ca --promote
                  make the pending CA current; the old CA certificate moves to DIR/retired/ and
                  the old key is deleted. The next "renew" reissues the server certificate.
  rotate-ca --abort
                  discard the pending CA
EOF
      ;;
    fingerprint)
      cat <<'EOF'
Print the SHA-256 fingerprint (over the DER certificate; "sha256sum ca.crt" does not match).

  --cert FILE     certificate to print (default DIR/ca.crt; DIR/next.crt is added when present)

Works with LibreSSL. Uses no lock and no key.
EOF
      ;;
    status)
      cat <<'EOF'
Show the CA and the server certificate. Read-only; needs no key and no lock.

  --out OUT           directory of the server certificate (default DIR/server)
  --threshold-days N  threshold used for the "renewal" line (default 60)
EOF
      ;;
    config-hint)
      cat <<'EOF'
Print copy-paste settings for sylvode.toml, nginx and Caddy. Read-only; contains no secret.

  --out OUT            directory of the certificate files (default DIR/server)
  --name NAME          primary host (default: first name of the CA)
  --port N             HTTPS port (default 443)
  --http-port N        port of the redirect server, 0 = none (default 80)
  --upstream HOST:PORT Sylvode web UI (default 127.0.0.1:3000)
  --proxy nginx|caddy|all  which proxy part to print (default all)
  --cert-path PATH     certificate chain as the proxy sees it (default OUT/fullchain.pem)
  --key-path PATH      private key as the proxy sees it (default OUT/server.key)
EOF
      ;;
  esac
}

usage_error() {
  err "$1"
  printf 'usage: %s\n' "$(usage_line "$SUB")" >&2
  exit 2
}

# ---------------------------------------------------------------------------------------------
# Option parsing (--opt value and --opt=value; any order after the subcommand)
# ---------------------------------------------------------------------------------------------

O_DIR=
O_OPENSSL=
O_QUIET=
O_CA_DAYS=
O_CA_NAME=
O_OUT=
O_OWNER=
O_DAYS=
O_FORCE=
O_EXEC=
O_THRESHOLD_DAYS=
O_CHECK=
O_PROMOTE=
O_ABORT=
O_CERT=
O_NAME=
O_PORT=
O_HTTP_PORT=
O_UPSTREAM=
O_PROXY=
O_CERT_PATH=
O_KEY_PATH=
VALOPTS=
FLAGOPTS=
SEEN=' '

parse_opts() {
  while [ $# -gt 0 ]; do
    _po_a=$1
    shift
    case $_po_a in
      --help | -h)
        help_sub
        exit 0
        ;;
      --*=*)
        _po_o=${_po_a%%=*}
        _po_v=${_po_a#*=}
        _po_hv=1
        ;;
      --*)
        _po_o=$_po_a
        _po_v=
        _po_hv=0
        ;;
      *) usage_error "unexpected argument: $(safe_str "$_po_a")" ;;
    esac
    _po_n=${_po_o#--}
    _po_k=
    case " $VALOPTS " in
      *" $_po_n "*) _po_k=opt ;;
    esac
    if [ -z "$_po_k" ]; then
      case " $FLAGOPTS " in
        *" $_po_n "*) _po_k=flag ;;
      esac
    fi
    [ -n "$_po_k" ] || usage_error "unknown option: $(safe_str "$_po_o")"
    _po_var=O_$(printf '%s' "$_po_n" | tr 'a-z-' 'A-Z_')
    if [ "$_po_k" = flag ]; then
      [ "$_po_hv" = 0 ] || usage_error "option $_po_o takes no value"
      eval "$_po_var=1"
      continue
    fi
    if [ "$_po_hv" = 0 ]; then
      [ $# -gt 0 ] || usage_error "missing value for $_po_o"
      _po_v=$1
      shift
    fi
    case $_po_n in
      ip) add_name ip "$_po_v" ;;
      dns) add_name dns "$_po_v" ;;
      *)
        case $SEEN in
          *" $_po_n "*) usage_error "option $_po_o given more than once" ;;
        esac
        SEEN="$SEEN$_po_n "
        eval "$_po_var=\$_po_v"
        ;;
    esac
  done
  if [ "$O_QUIET" = 1 ]; then
    QUIET=1
  fi
}

# ---------------------------------------------------------------------------------------------
# Names (contract 4.3). Every name is validated before it reaches a config file or a shell word.
# ---------------------------------------------------------------------------------------------

# oct_ok VALUE: a decimal octet 0-255 without leading zeros.
oct_ok() {
  case $1 in
    "" | *[!0-9]*) return 1 ;;
    0) return 0 ;;
    0*) return 1 ;;
  esac
  [ "${#1}" -le 3 ] && [ "$1" -le 255 ]
}

# canon_ip4 ADDR: prints the address; returns 2 when refused.
canon_ip4() {
  case $1 in
    "" | *[!0-9.]* | .* | *. | *..*)
      err "invalid IPv4 address: $(safe_str "$1")"
      return 2
      ;;
  esac
  IFS=. read -r _c4_a _c4_b _c4_c _c4_d _c4_x <<EOF
$1
EOF
  if [ -n "$_c4_x" ] || ! oct_ok "$_c4_a" || ! oct_ok "$_c4_b" || ! oct_ok "$_c4_c" || ! oct_ok "$_c4_d"; then
    err "invalid IPv4 address (four decimal octets 0-255 without leading zeros): $(safe_str "$1")"
    return 2
  fi
  if [ "$1" = 0.0.0.0 ] || [ "$1" = 255.255.255.255 ]; then
    err "refused IPv4 address: $1"
    return 2
  fi
  if [ "$_c4_a" -ge 224 ] && [ "$_c4_a" -le 239 ]; then
    err "refused IPv4 multicast address: $1"
    return 2
  fi
  _c4_priv=0
  case $_c4_a in
    10 | 127) _c4_priv=1 ;;
    172) [ "$_c4_b" -ge 16 ] && [ "$_c4_b" -le 31 ] && _c4_priv=1 ;;
    192) [ "$_c4_b" = 168 ] && _c4_priv=1 ;;
    100) [ "$_c4_b" -ge 64 ] && [ "$_c4_b" -le 127 ] && _c4_priv=1 ;;
    169) [ "$_c4_b" = 254 ] && _c4_priv=1 ;;
  esac
  if [ "$_c4_priv" != 1 ]; then
    warnn "$1 is not a private address (10/8, 172.16/12, 192.168/16, 100.64/10, 127/8, 169.254/16)"
  fi
  printf '%s\n' "$1"
}

# canon_ip6 ADDR: prints the lowercase address; returns 2 when refused.
canon_ip6() {
  _c6_s=$(printf '%s' "$1" | tr 'A-F' 'a-f')
  _c6_bad=0
  case $_c6_s in
    "" | *[!0-9a-f:]*) _c6_bad=1 ;;
    *:::*) _c6_bad=1 ;;
    :[!:]* | *[!:]:) _c6_bad=1 ;;
  esac
  _c6_dc=0
  if [ "$_c6_bad" = 0 ]; then
    case $_c6_s in
      *::*)
        _c6_dc=1
        _c6_rest=${_c6_s#*::}
        case $_c6_rest in
          *::*) _c6_bad=1 ;;
        esac
        ;;
    esac
  fi
  if [ "$_c6_bad" = 0 ]; then
    _c6_colons=$(printf '%s' "$_c6_s" | tr -cd ':')
    [ "${#_c6_colons}" -ge 2 ] || _c6_bad=1
  fi
  _c6_n=0
  if [ "$_c6_bad" = 0 ]; then
    _c6_list=$(printf '%s\n' "$_c6_s" | tr ':' '\n')
    while IFS= read -r _c6_g; do
      [ -n "$_c6_g" ] || continue
      if [ "${#_c6_g}" -gt 4 ]; then
        _c6_bad=1
        break
      fi
      _c6_n=$((_c6_n + 1))
    done <<EOF
$_c6_list
EOF
    if [ "$_c6_dc" = 1 ]; then
      [ "$_c6_n" -le 7 ] || _c6_bad=1
    else
      [ "$_c6_n" -eq 8 ] || _c6_bad=1
    fi
  fi
  if [ "$_c6_bad" = 1 ]; then
    err "invalid IPv6 address (hex groups and at most one \"::\"; no zone, no dotted IPv4 form): $(safe_str "$1")"
    return 2
  fi
  if [ "$_c6_s" = "::" ]; then
    err "refused IPv6 address: ::"
    return 2
  fi
  _c6_first=${_c6_s%%:*}
  case $_c6_first in
    ff??)
      err "refused IPv6 multicast address: $_c6_s"
      return 2
      ;;
  esac
  case $_c6_s in
    fc??:* | fd??:* | fe[89ab]?:* | ::1) ;;
    *) warnn "$_c6_s is not a private IPv6 address (fc00::/7, fe80::/10, ::1)" ;;
  esac
  printf '%s\n' "$_c6_s"
}

canon_ip() {
  case $1 in
    *:*) canon_ip6 "$1" ;;
    *) canon_ip4 "$1" ;;
  esac
}

# canon_dns NAME: prints the lowercase name; returns 2 when refused.
canon_dns() {
  case $1 in
    "" | *[!a-zA-Z0-9.-]*)
      err "invalid DNS name (letters, digits, \".\" and \"-\" only; no wildcards): $(safe_str "$1")"
      return 2
      ;;
  esac
  _cd_d=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
  case $_cd_d in
    .* | *. | *..* | -* | *-.* | *.-* | *-)
      err "invalid DNS name (empty label, trailing dot, or label edge \"-\"): $(safe_str "$1")"
      return 2
      ;;
  esac
  if [ "${#_cd_d}" -gt 253 ]; then
    err "DNS name longer than 253 characters: $(safe_str "$1")"
    return 2
  fi
  case $_cd_d in
    *[!0-9.]*) ;;
    *)
      err "a DNS name must not look like an IP address: $(safe_str "$1"); use --ip"
      return 2
      ;;
  esac
  _cd_bad=0
  _cd_labels=$(printf '%s\n' "$_cd_d" | tr '.' '\n')
  while IFS= read -r _cd_l; do
    if [ "${#_cd_l}" -lt 1 ] || [ "${#_cd_l}" -gt 63 ]; then
      _cd_bad=1
    fi
  done <<EOF
$_cd_labels
EOF
  if [ "$_cd_bad" = 1 ]; then
    err "invalid DNS name (labels are 1-63 characters): $(safe_str "$1")"
    return 2
  fi
  if [ "${_cd_d##*.}" = invalid ]; then
    err "the label \"invalid\" is reserved for the CA's placeholder: $(safe_str "$1")"
    return 2
  fi
  case $_cd_d in
    *.*) ;;
    *)
      err "single-label DNS name \"$_cd_d\": use at least two labels (a single label would cover everything below it)"
      return 2
      ;;
  esac
  printf '%s\n' "$_cd_d"
}

# list_has LIST ITEM
list_has() {
  printf '%s\n' "$1" | grep -qxF -e "$2"
}

# list_subset A B: every item of A is in B.
list_subset() {
  printf '%s\n' "$1" | while IFS= read -r _ls_i; do
    [ -z "$_ls_i" ] || list_has "$2" "$_ls_i" || exit 1
  done
}

list_equal() {
  list_subset "$1" "$2" && list_subset "$2" "$1"
}

# join_list LIST SEPARATOR
join_list() {
  _jl_out=
  while IFS= read -r _jl_i; do
    [ -n "$_jl_i" ] || continue
    if [ -n "$_jl_out" ]; then
      _jl_out=$_jl_out$2$_jl_i
    else
      _jl_out=$_jl_i
    fi
  done <<EOF
$1
EOF
  printf '%s\n' "$_jl_out"
}

# add_name ip|dns RAW: validates and appends "IP:x" / "DNS:y" to NAMES (called while parsing).
add_name() {
  if [ "$1" = ip ]; then
    _an_v=$(canon_ip "$2") || exit 2
    _an_item=IP:$_an_v
  else
    _an_v=$(canon_dns "$2") || exit 2
    _an_item=DNS:$_an_v
  fi
  if list_has "$NAMES" "$_an_item"; then
    warn "duplicate name ignored: $_an_item"
    return 0
  fi
  NAMES=${NAMES}${_an_item}${NL}
}

# name_item_ok ITEM: the item is canonical ("IP:..." or "DNS:...").
name_item_ok() {
  _ni_old=$NOWARN
  NOWARN=1
  _ni_rc=1
  case $1 in
    IP:*)
      if _ni_v=$(canon_ip "${1#IP:}" 2>/dev/null) && [ "$_ni_v" = "${1#IP:}" ]; then
        _ni_rc=0
      fi
      ;;
    DNS:*)
      if _ni_v=$(canon_dns "${1#DNS:}" 2>/dev/null) && [ "$_ni_v" = "${1#DNS:}" ]; then
        _ni_rc=0
      fi
      ;;
  esac
  NOWARN=$_ni_old
  return "$_ni_rc"
}

# read_names FILE: sets READ_NAMES (validated, one per line); dies with 3 on a bad entry.
read_names() {
  READ_NAMES=
  while IFS= read -r _rn_i; do
    [ -n "$_rn_i" ] || continue
    name_item_ok "$_rn_i" || die 3 "$1 holds an invalid entry; refusing to continue"
    READ_NAMES=${READ_NAMES}${_rn_i}${NL}
  done <"$1"
  [ -n "$READ_NAMES" ] || die 3 "$1 holds no names"
}

# ---------------------------------------------------------------------------------------------
# IPv6 helpers for the name-constraint config
# ---------------------------------------------------------------------------------------------

# ip6_strip ADDR8: 8 colon-separated groups, any case, leading zeros removed, lowercase.
ip6_strip() {
  printf '%s\n' "$1" | tr 'A-F' 'a-f' | tr ':' '\n' | while IFS= read -r _is_g; do
    while :; do
      case $_is_g in
        0?*) _is_g=${_is_g#0} ;;
        *) break ;;
      esac
    done
    printf '%s\n' "$_is_g"
  done | tr '\n' ':' | sed 's/:$//'
}

# ip6_expand ADDR: canonical lowercase address -> eight unpadded groups.
ip6_expand() {
  case $1 in
    *::*)
      _ie_l=${1%%::*}
      _ie_r=${1#*::}
      _ie_nl=$(printf '%s' "$_ie_l" | tr ':' '\n' | grep -c .)
      _ie_nr=$(printf '%s' "$_ie_r" | tr ':' '\n' | grep -c .)
      _ie_fill=$((8 - _ie_nl - _ie_nr))
      _ie_out=$_ie_l
      _ie_i=0
      while [ "$_ie_i" -lt "$_ie_fill" ]; do
        _ie_out=${_ie_out:+$_ie_out:}0
        _ie_i=$((_ie_i + 1))
      done
      if [ -n "$_ie_r" ]; then
        _ie_out=${_ie_out:+$_ie_out:}$_ie_r
      fi
      ip6_strip "$_ie_out"
      ;;
    *) ip6_strip "$1" ;;
  esac
}

IP6_FULL_MASK=ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff

# ---------------------------------------------------------------------------------------------
# OpenSSL selection (SD-12)
# ---------------------------------------------------------------------------------------------

find_openssl() {
  OPENSSL=${O_OPENSSL:-${OPENSSL:-openssl}}
  case $OPENSSL in
    */*) [ -x "$OPENSSL" ] || die 4 "OpenSSL binary not found: $OPENSSL" ;;
    *) command -v "$OPENSSL" >/dev/null 2>&1 || die 4 "OpenSSL not found on PATH; install OpenSSL 3 or pass --openssl PATH" ;;
  esac
  SSL_VERSION=$("$OPENSSL" version 2>/dev/null | head -n 1)
  [ -n "$SSL_VERSION" ] || die 4 "cannot run $OPENSSL"
  SSL_OK=0
  SSL_LIBRE=0
  case $SSL_VERSION in
    "OpenSSL 3."* | "OpenSSL "[4-9]* | "OpenSSL "[1-9][0-9]*) SSL_OK=1 ;;
    LibreSSL*) SSL_LIBRE=1 ;;
  esac
}

# need_openssl3: refuse LibreSSL and OpenSSL older than 3.0 (exit 4).
need_openssl3() {
  find_openssl
  if [ "$SSL_OK" != 1 ]; then
    die 4 "OpenSSL 3.0 or newer is required (found: $SSL_VERSION). LibreSSL mis-verifies IP name constraints. On macOS install openssl@3 with Homebrew and pass --openssl /opt/homebrew/opt/openssl@3/bin/openssl (Apple Silicon) or --openssl /usr/local/opt/openssl@3/bin/openssl (Intel), or set OPENSSL."
  fi
}

# need_openssl_read: LibreSSL is accepted (fingerprint and status only), with a warning.
need_openssl_read() {
  find_openssl
  if [ "$SSL_OK" != 1 ]; then
    if [ "$SSL_LIBRE" = 1 ]; then
      warn "$SSL_VERSION is only trusted for reading; issuing and verifying need OpenSSL 3 (see --help)"
    else
      die 4 "OpenSSL 3.0 or newer is required (found: $SSL_VERSION)"
    fi
  fi
}

# ---------------------------------------------------------------------------------------------
# Path safety (SD-9) and permission self-check (SD-10)
# ---------------------------------------------------------------------------------------------

# dir_ok DIR: owned by uid 0 or the invoking uid, not group- or world-writable.
dir_ok() {
  find_match "$1" -prune -type d \( -user 0 -o -user "$SELF_UID" \) ! -perm -020 ! -perm -002
}

# check_chain PHYSICAL-DIR: every component from / downwards passes dir_ok.
check_chain() {
  _cc_c=$1
  while :; do
    dir_ok "$_cc_c" || die 3 "insecure path component $_cc_c (it must be owned by root or by the invoking user and not group- or world-writable); choose a root-controlled directory"
    [ "$_cc_c" != / ] || break
    _cc_c=${_cc_c%/*}
    [ -n "$_cc_c" ] || _cc_c=/
  done
}

# resolve_dir PATH: sets RES_BASE (physical, existing) and RES_TAIL (missing components, or empty).
resolve_dir() {
  _rd_p=$1
  case $_rd_p in
    /*) ;;
    *) _rd_p=$PWD/$_rd_p ;;
  esac
  case "$_rd_p/" in
    */./* | */../*) die 2 "the path must not contain \".\" or \"..\" components: $(safe_str "$1")" ;;
  esac
  while :; do
    case $_rd_p in
      */) _rd_p=${_rd_p%/} ;;
      *) break ;;
    esac
  done
  _rd_tail=
  while [ -n "$_rd_p" ] && [ ! -d "$_rd_p" ]; do
    if [ -e "$_rd_p" ] || [ -L "$_rd_p" ]; then
      die 3 "$_rd_p exists and is not a directory"
    fi
    _rd_tail=/${_rd_p##*/}$_rd_tail
    _rd_p=${_rd_p%/*}
  done
  [ -n "$_rd_p" ] || _rd_p=/
  RES_BASE=$(cd -P "$_rd_p" 2>/dev/null && pwd -P) || die 3 "cannot resolve $_rd_p"
  RES_TAIL=$_rd_tail
}

# secure_dir PATH CREATE: resolves, checks SD-9, creates missing components (CREATE=1, mode 0755)
# or dies with 4 (CREATE=0). Sets SECURED to the physical path.
secure_dir() {
  resolve_dir "$1"
  check_chain "$RES_BASE"
  if [ -n "$RES_TAIL" ]; then
    [ "$2" = 1 ] || die 4 "$1 does not exist"
    _sd_cur=$RES_BASE
    _sd_rest=${RES_TAIL#/}
    while [ -n "$_sd_rest" ]; do
      _sd_comp=${_sd_rest%%/*}
      case $_sd_rest in
        */*) _sd_rest=${_sd_rest#*/} ;;
        *) _sd_rest= ;;
      esac
      if [ "$_sd_cur" = / ]; then
        _sd_cur=/$_sd_comp
      else
        _sd_cur=$_sd_cur/$_sd_comp
      fi
      mkdir -m 0755 "$_sd_cur" || die 1 "cannot create directory $_sd_cur"
    done
    _sd_phys=$(cd -P "$_sd_cur" 2>/dev/null && pwd -P) || die 3 "cannot resolve $_sd_cur"
    check_chain "$_sd_phys"
    SECURED=$_sd_phys
  else
    SECURED=$RES_BASE
  fi
}

# plain_dir PATH: read-only commands; no SD-9 check. Sets SECURED (absolute, resolved when present).
plain_dir() {
  resolve_dir "$1"
  if [ "$RES_BASE" = / ]; then
    SECURED=$RES_TAIL
  else
    SECURED=$RES_BASE$RES_TAIL
  fi
}

# check_key_perms KEYDIR: SD-10 for KEYDIR/ca.key.
check_key_perms() {
  find_match "$1" -prune -type d -user "$SELF_UID" -perm 0700 ||
    die 3 "insecure permissions on $1 (need mode 0700, owned by uid $SELF_UID)"
  find_match "$1/ca.key" -prune -type f -user "$SELF_UID" -perm 0400 ||
    die 3 "insecure permissions on $1/ca.key (need mode 0400, owned by uid $SELF_UID)"
}

# ---------------------------------------------------------------------------------------------
# Locking
# ---------------------------------------------------------------------------------------------

acquire_lock() {
  _al_lk=$DIR/.lock
  if ! mkdir "$_al_lk" 2>/dev/null; then
    _al_pid=$(sed -n '1p' "$_al_lk/pid" 2>/dev/null)
    if [ -z "$_al_pid" ]; then
      sleep 1
      _al_pid=$(sed -n '1p' "$_al_lk/pid" 2>/dev/null)
    fi
    case $_al_pid in
      *[!0-9]*) _al_pid= ;;
    esac
    if [ -n "$_al_pid" ] && kill -0 "$_al_pid" 2>/dev/null; then
      die 6 "another instance holds the lock ($_al_lk, pid $_al_pid)"
    fi
    warn "removing stale lock $_al_lk"
    # Take the stale lock over by renaming it to a name only this process uses, so a competing
    # instance cannot delete the lock this one is about to create. If the renamed lock is not the
    # stale one seen above, another instance replaced it first: leave it (restore it when the
    # name is still free) and give up.
    _al_old=$_al_lk.stale.$$
    rm -rf "$_al_old"
    mv "$_al_lk" "$_al_old" 2>/dev/null || die 6 "another instance holds the lock ($_al_lk)"
    _al_now=$(sed -n '1p' "$_al_old/pid" 2>/dev/null)
    if [ "$_al_now" != "$_al_pid" ]; then
      [ -e "$_al_lk" ] || mv "$_al_old" "$_al_lk" 2>/dev/null || :
      die 6 "another instance holds the lock ($_al_lk)"
    fi
    rm -rf "$_al_old"
    mkdir "$_al_lk" 2>/dev/null || die 6 "another instance holds the lock ($_al_lk)"
  fi
  printf '%s\n' "$$" >"$_al_lk/pid"
  LOCK_HELD=1
}

# ---------------------------------------------------------------------------------------------
# Certificate readers
# ---------------------------------------------------------------------------------------------

# cert_fp FILE: SHA-256 fingerprint, uppercase, colon-separated (SD-18).
cert_fp() {
  "$OPENSSL" x509 -in "$1" -noout -fingerprint -sha256 2>/dev/null | sed -n 's/^.*Fingerprint=//p' | head -n 1 | tr 'a-f' 'A-F'
}

fp16() {
  printf '%s' "$1" | tr -d ':' | tr 'A-F' 'a-f' | cut -c1-16
}

cert_subject() {
  "$OPENSSL" x509 -in "$1" -noout -subject -nameopt RFC2253 2>/dev/null | sed -n 's/^subject= *//p' | sed 's/,/, /g'
}

cert_enddate() {
  "$OPENSSL" x509 -in "$1" -noout -enddate 2>/dev/null | sed -n 's/^notAfter=//p'
}

cert_serial() {
  "$OPENSSL" x509 -in "$1" -noout -serial 2>/dev/null | sed -n 's/^serial=//p'
}

# cert_keyid FILE "Subject Key Identifier"|"Authority Key Identifier"
cert_keyid() {
  "$OPENSSL" x509 -in "$1" -noout -text 2>/dev/null |
    sed -n "/X509v3 $2:/{n;s/^ *//;s/^keyid://;p;q;}"
}

# key_matches KEY CERT: the private key belongs to the certificate (public keys compared; nothing printed).
key_matches() {
  _km_a=$("$OPENSSL" pkey -in "$1" -pubout 2>/dev/null) || return 1
  _km_b=$("$OPENSSL" x509 -in "$2" -pubkey -noout 2>/dev/null) || return 1
  [ -n "$_km_a" ] && [ "$_km_a" = "$_km_b" ]
}

# issuer_current CERT: signed by the current CA (verification with OpenSSL 3, key ids otherwise).
issuer_current() {
  if [ "$SSL_OK" = 1 ]; then
    "$OPENSSL" verify -CAfile "$DIR/ca.crt" -purpose sslserver "$1" >/dev/null 2>&1
  else
    _ic_a=$(cert_keyid "$1" "Authority Key Identifier")
    _ic_s=$(cert_keyid "$DIR/ca.crt" "Subject Key Identifier")
    [ -n "$_ic_a" ] && [ "$_ic_a" = "$_ic_s" ]
  fi
}

# ca_names_ok: DIR/ca.names read into CA_NAMES.
load_ca_names() {
  read_names "$DIR/ca.names"
  CA_NAMES=$READ_NAMES
}

# read_issue_conf OUT: sets IC_NAMES (list), IC_OWNER, IC_DAYS (all re-validated).
read_issue_conf() {
  _rc_f=$1/issue.conf
  [ -f "$_rc_f" ] || die 4 "$_rc_f is missing; nothing to renew, run issue"
  _rc_v=$(sed -n 's/^version=//p' "$_rc_f" | head -n 1)
  [ "$_rc_v" = 1 ] || die 3 "$_rc_f: unsupported version"
  _rc_n=$(sed -n 's/^names=//p' "$_rc_f" | head -n 1)
  IC_NAMES=
  for _rc_i in $(printf '%s' "$_rc_n" | tr -c 'A-Za-z0-9.:_-' ' '); do
    name_item_ok "$_rc_i" || die 3 "$_rc_f: invalid name entry"
    IC_NAMES=${IC_NAMES}${_rc_i}${NL}
  done
  [ -n "$IC_NAMES" ] || die 3 "$_rc_f: no names"
  IC_OWNER=$(sed -n 's/^owner=//p' "$_rc_f" | head -n 1)
  case $IC_OWNER in
    *[!A-Za-z0-9._:-]*) die 3 "$_rc_f: invalid owner" ;;
  esac
  IC_DAYS=$(sed -n 's/^days=//p' "$_rc_f" | head -n 1)
  is_int "$IC_DAYS" 1 825 || die 3 "$_rc_f: invalid days"
}

# ---------------------------------------------------------------------------------------------
# CA generation (contract 5.1, 6.1)
# ---------------------------------------------------------------------------------------------

# write_ca_cnf FILE NAMES CN
write_ca_cnf() {
  {
    printf '[req]\ndistinguished_name = dn\nprompt = no\nutf8 = yes\nstring_mask = utf8only\n'
    printf '[dn]\nO = Sylvode intranet\nCN = %s\n' "$3"
    printf '[ca]\nbasicConstraints = critical, CA:TRUE, pathlen:0\n'
    printf 'keyUsage = critical, keyCertSign, cRLSign\nextendedKeyUsage = serverAuth\n'
    printf 'subjectKeyIdentifier = hash\nauthorityKeyIdentifier = keyid:always\n'
    printf 'nameConstraints = critical, @nc\n[nc]\n'
    _wc_ip=0
    _wc_dns=0
    while IFS= read -r _wc_i; do
      case $_wc_i in
        IP:*:*)
          printf 'permitted;IP.%s = %s/%s\n' "$_wc_ip" "$(ip6_expand "${_wc_i#IP:}")" "$IP6_FULL_MASK"
          _wc_ip=$((_wc_ip + 1))
          ;;
        IP:*)
          printf 'permitted;IP.%s = %s/255.255.255.255\n' "$_wc_ip" "${_wc_i#IP:}"
          _wc_ip=$((_wc_ip + 1))
          ;;
        DNS:*)
          printf 'permitted;DNS.%s = %s\n' "$_wc_dns" "${_wc_i#DNS:}"
          _wc_dns=$((_wc_dns + 1))
          ;;
      esac
    done <<EOF
$2
EOF
    if [ "$_wc_dns" = 0 ]; then
      printf 'permitted;DNS.0 = invalid\n'
    fi
    if [ "$_wc_ip" = 0 ]; then
      printf 'permitted;IP.0 = 255.255.255.255/255.255.255.255\n'
    fi
  } >"$1"
}

# nc_norm: normalise "IP:a/b" and "DNS:x" lines of a nameConstraints listing for comparison.
nc_norm() {
  while IFS= read -r _nn_l; do
    [ -n "$_nn_l" ] || continue
    case $_nn_l in
      IP:*:*/*)
        _nn_a=${_nn_l#IP:}
        printf 'IP:%s/%s\n' "$(ip6_strip "${_nn_a%%/*}")" "$(ip6_strip "${_nn_a#*/}")"
        ;;
      IP:*) printf '%s\n' "$_nn_l" ;;
      DNS:*) printf 'DNS:%s\n' "$(printf '%s' "${_nn_l#DNS:}" | tr '[:upper:]' '[:lower:]')" ;;
    esac
  done
}

# expected_nc NAMES: the permitted subtrees the CA must carry.
expected_nc() {
  _en_ip=0
  _en_dns=0
  while IFS= read -r _en_i; do
    case $_en_i in
      IP:*:*)
        printf 'IP:%s/%s\n' "$(ip6_expand "${_en_i#IP:}")" "$IP6_FULL_MASK"
        _en_ip=1
        ;;
      IP:*)
        printf 'IP:%s/255.255.255.255\n' "${_en_i#IP:}"
        _en_ip=1
        ;;
      DNS:*)
        printf '%s\n' "$_en_i"
        _en_dns=1
        ;;
    esac
  done <<EOF
$1
EOF
  [ "$_en_dns" = 1 ] || printf 'DNS:invalid\n'
  [ "$_en_ip" = 1 ] || printf 'IP:255.255.255.255/255.255.255.255\n'
}

# make_ca KEYDIR CRT NAMESFILE NAMES CN DAYS: generates and installs a CA (key, names, then certificate).
make_ca() {
  _mc_keydir=$1
  _mc_crt=$2
  _mc_namesf=$3
  _mc_names=$4
  _mc_cn=$5
  _mc_days=$6
  # An existing key directory is only reused when it is a real directory, owned by the invoking
  # user and mode 0700: the new key must not be written where somebody else can replace it.
  if [ -e "$_mc_keydir" ] || [ -L "$_mc_keydir" ]; then
    find_match "$_mc_keydir" -prune -type d -user "$SELF_UID" -perm 0700 ||
      die 3 "refusing the existing key directory $_mc_keydir: it must be a real directory (not a symlink) owned by uid $SELF_UID with mode 0700"
  fi
  mk_tmp "$DIR"
  _mc_t=$TMPD
  "$OPENSSL" genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$_mc_t/ca.key" 2>/dev/null ||
    die 1 "step failed: generating the CA key"
  write_ca_cnf "$_mc_t/ca.cnf" "$_mc_names" "$_mc_cn" || die 1 "step failed: writing the CA configuration"
  _mc_serial=0x$("$OPENSSL" rand -hex 16) || die 1 "step failed: generating the serial number"
  "$OPENSSL" req -new -x509 -key "$_mc_t/ca.key" -sha256 -days "$_mc_days" -set_serial "$_mc_serial" \
    -config "$_mc_t/ca.cnf" -extensions ca -out "$_mc_t/ca.crt" ||
    die 1 "step failed: signing the CA certificate"
  # Self-check: the name constraints are exactly the expected ones, and the CA verifies itself.
  _mc_ext=$("$OPENSSL" x509 -in "$_mc_t/ca.crt" -noout -ext nameConstraints 2>/dev/null) ||
    die 1 "step failed: reading the CA name constraints"
  case $_mc_ext in
    *"Name Constraints: critical"*) ;;
    *) die 1 "self-check failed: the name constraints are not critical" ;;
  esac
  _mc_got=$(printf '%s\n' "$_mc_ext" | sed -n 's/^ *\(IP:.*\)$/\1/p;s/^ *\(DNS:.*\)$/\1/p' | nc_norm)
  _mc_want=$(expected_nc "$_mc_names" | nc_norm)
  list_equal "$_mc_got" "$_mc_want" || die 1 "self-check failed: the name constraints differ from the requested names"
  "$OPENSSL" verify -CAfile "$_mc_t/ca.crt" "$_mc_t/ca.crt" >/dev/null 2>&1 ||
    die 1 "self-check failed: the CA does not verify itself"
  printf '%s' "$_mc_names" >"$_mc_t/ca.names" || die 1 "step failed: writing the name list"
  chmod 0400 "$_mc_t/ca.key"
  chmod 0644 "$_mc_t/ca.crt" "$_mc_t/ca.names"
  if [ ! -e "$_mc_keydir" ] && [ ! -L "$_mc_keydir" ]; then
    mkdir -m 0700 "$_mc_keydir" || die 1 "cannot create $_mc_keydir"
  fi
  mv -f "$_mc_t/ca.key" "$_mc_keydir/ca.key" || die 1 "cannot install the CA key"
  mv -f "$_mc_t/ca.names" "$_mc_namesf" || die 1 "cannot install the name list"
  mv -f "$_mc_t/ca.crt" "$_mc_crt" || die 1 "cannot install the CA certificate"
  rm -rf "$_mc_t"
}

# permitted_text NAMES: operator-facing description of the permitted names.
print_permitted() {
  _pp_ip=0
  _pp_dns=0
  while IFS= read -r _pp_i; do
    case $_pp_i in
      IP:*) _pp_ip=1 ;;
      DNS:*) _pp_dns=1 ;;
    esac
  done <<EOF
$1
EOF
  printf 'Permitted names: %s\n' "$(join_list "$1" ", ")"
  if [ "$_pp_dns" = 0 ]; then
    printf '%s\n' '                 (no DNS names: the CA is limited to the placeholder "invalid")'
  fi
  if [ "$_pp_ip" = 0 ]; then
    printf '%s\n' '                 (no IP addresses: the CA is limited to the placeholder 255.255.255.255)'
  fi
}

# print_ca_block CRT NAMES KEYDIR
print_ca_block() {
  printf 'CA certificate:  %s\n' "$1"
  printf 'Subject:         %s\n' "$(cert_subject "$1")"
  printf 'Valid until:     %s\n' "$(cert_enddate "$1")"
  print_permitted "$2"
  printf 'SHA-256:         %s\n' "$(cert_fp "$1")"
  printf 'Distribute only the CA certificate. The CA key stays in %s.\n' "$3"
}

# ---------------------------------------------------------------------------------------------
# Server certificate (contract 5.2, 6.2)
# ---------------------------------------------------------------------------------------------

# ca_ok_days DAYS: the CA is still valid for DAYS days.
ca_valid_days() {
  "$OPENSSL" x509 -in "$DIR/ca.crt" -noout -checkend $(($1 * 86400)) >/dev/null 2>&1
}

# cap_days DAYS: sets EFF_DAYS (reduced when the CA expires first).
cap_days() {
  EFF_DAYS=$1
  if ca_valid_days "$1"; then
    return 0
  fi
  _cp_lo=0
  _cp_hi=$1
  while [ $((_cp_hi - _cp_lo)) -gt 1 ]; do
    _cp_mid=$(((_cp_lo + _cp_hi) / 2))
    if ca_valid_days "$_cp_mid"; then
      _cp_lo=$_cp_mid
    else
      _cp_hi=$_cp_mid
    fi
  done
  [ "$_cp_lo" -ge 1 ] || die 3 "the CA expires within a day; run rotate-ca"
  EFF_DAYS=$_cp_lo
  warn "leaf lifetime capped to $_cp_lo days by the CA's expiry; rotate the CA"
}

# parse_owner SPEC: sets OWN_USER, OWN_GROUP, OWNER_SPEC (resolved "user:group"); exit 4 when unknown.
parse_owner() {
  OWNER_SPEC=
  [ -n "$1" ] || return 0
  [ "$SELF_UID" = 0 ] || die 4 "--owner needs root"
  case $1 in
    *[!A-Za-z0-9._:-]* | -* | :* | *:*:*) die 4 "invalid owner: $(safe_str "$1")" ;;
  esac
  OWN_USER=${1%%:*}
  case $1 in
    *:*) OWN_GROUP=${1#*:} ;;
    *) OWN_GROUP= ;;
  esac
  [ -n "$OWN_USER" ] || die 4 "invalid owner: $(safe_str "$1")"
  id -u "$OWN_USER" >/dev/null 2>&1 || die 4 "unknown user: $OWN_USER"
  if [ -z "$OWN_GROUP" ]; then
    OWN_GROUP=$(id -gn "$OWN_USER" 2>/dev/null) || die 4 "cannot determine the group of $OWN_USER"
  fi
  mk_tmp "$DIR"
  : >"$TMPD/owner-probe"
  chown "$OWN_USER:$OWN_GROUP" "$TMPD/owner-probe" 2>/dev/null || die 4 "unknown user or group: $OWN_USER:$OWN_GROUP"
  rm -rf "$TMPD"
  OWNER_SPEC=$OWN_USER:$OWN_GROUP
}

# issue_cert NAMES OUT DAYS: creates a key, signs, verifies, installs atomically (SD-15).
issue_cert() {
  _is_names=$1
  _is_out=$2
  cap_days "$3"
  if ! ca_valid_days 400; then
    warn "the CA expires within 400 days: run rotate-ca soon"
  fi
  mk_tmp "$_is_out"
  _is_t=$TMPD
  "$OPENSSL" genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$_is_t/server.key" 2>/dev/null ||
    die 1 "step failed: generating the server key"
  printf '[req]\ndistinguished_name = dn\nprompt = no\nutf8 = yes\nstring_mask = utf8only\n[dn]\nO = Sylvode intranet server\n' >"$_is_t/csr.cnf"
  "$OPENSSL" req -new -key "$_is_t/server.key" -config "$_is_t/csr.cnf" -out "$_is_t/server.csr" ||
    die 1 "step failed: creating the certificate request"
  {
    printf '[leaf]\nbasicConstraints = critical, CA:FALSE\nkeyUsage = critical, digitalSignature\n'
    printf 'extendedKeyUsage = serverAuth\nsubjectAltName = @san\n'
    printf 'authorityKeyIdentifier = keyid\nsubjectKeyIdentifier = hash\n[san]\n'
    _is_ip=0
    _is_dns=0
    while IFS= read -r _is_i; do
      case $_is_i in
        IP:*)
          printf 'IP.%s = %s\n' "$_is_ip" "${_is_i#IP:}"
          _is_ip=$((_is_ip + 1))
          ;;
        DNS:*)
          printf 'DNS.%s = %s\n' "$_is_dns" "${_is_i#DNS:}"
          _is_dns=$((_is_dns + 1))
          ;;
      esac
    done <<EOF
$_is_names
EOF
  } >"$_is_t/leaf.cnf"
  _is_serial=0x$("$OPENSSL" rand -hex 16) || die 1 "step failed: generating the serial number"
  "$OPENSSL" x509 -req -in "$_is_t/server.csr" -CA "$DIR/ca.crt" -CAkey "$DIR/ca/ca.key" \
    -set_serial "$_is_serial" -days "$EFF_DAYS" -sha256 -extfile "$_is_t/leaf.cnf" -extensions leaf \
    -out "$_is_t/server.crt" 2>/dev/null || die 1 "step failed: signing the server certificate"
  "$OPENSSL" verify -CAfile "$DIR/ca.crt" -purpose sslserver "$_is_t/server.crt" >/dev/null 2>&1 ||
    die 1 "step failed: the new certificate does not verify against the CA (nothing was replaced)"
  key_matches "$_is_t/server.key" "$_is_t/server.crt" ||
    die 1 "step failed: the new key does not match the new certificate (nothing was replaced)"
  cat "$_is_t/server.crt" "$DIR/ca.crt" >"$_is_t/fullchain.pem" || die 1 "step failed: writing the chain"
  {
    printf 'version=1\n'
    printf 'names=%s\n' "$(join_list "$_is_names" " ")"
    printf 'owner=%s\n' "$OWNER_SPEC"
    printf 'days=%s\n' "$REQ_DAYS"
  } >"$_is_t/issue.conf"
  chmod 0600 "$_is_t/server.key"
  chmod 0644 "$_is_t/server.crt" "$_is_t/fullchain.pem" "$_is_t/issue.conf"
  if [ -n "$OWNER_SPEC" ]; then
    chown "$OWNER_SPEC" "$_is_t/server.key" || die 1 "step failed: setting the owner of the server key"
  fi
  mv -f "$_is_t/server.key" "$_is_out/server.key" || die 1 "cannot install server.key"
  mv -f "$_is_t/server.crt" "$_is_out/server.crt" || die 1 "cannot install server.crt"
  mv -f "$_is_t/fullchain.pem" "$_is_out/fullchain.pem" || die 1 "cannot install fullchain.pem"
  mv -f "$_is_t/issue.conf" "$_is_out/issue.conf" || die 1 "cannot install issue.conf"
  rm -rf "$_is_t"
  info "certificate written to $_is_out (valid $EFF_DAYS days)"
}

# renewal_due OUT THRESHOLD: returns 0 when due; sets DUE_REASON. Needs IC_NAMES and CA_NAMES.
renewal_due() {
  DUE_REASON=
  _rd_crt=$1/server.crt
  if ! "$OPENSSL" x509 -in "$_rd_crt" -noout -checkend $(($2 * 86400)) >/dev/null 2>&1; then
    DUE_REASON="the certificate expires within $2 days (or is expired or unreadable)"
    return 0
  fi
  if ! issuer_current "$_rd_crt"; then
    DUE_REASON="the certificate is not signed by the current CA"
    return 0
  fi
  if [ -r "$1/server.key" ]; then
    if ! key_matches "$1/server.key" "$_rd_crt"; then
      DUE_REASON="server.key does not match server.crt"
      return 0
    fi
  else
    KEY_UNREADABLE=1
  fi
  if ! list_subset "$IC_NAMES" "$CA_NAMES"; then
    DUE_REASON="a name of the certificate is no longer permitted by the CA"
    return 0
  fi
  return 1
}

# run_hook OUT: creates .reload-pending, releases the lock, runs the hook (SD-16).
run_hook() {
  _rh_o=$1
  _rh_p=$_rh_o/.reload-pending
  if [ -L "$_rh_p" ]; then
    die 3 "$_rh_p is a symbolic link; refusing to write through it"
  fi
  if [ ! -e "$_rh_p" ]; then
    (
      set -C
      : >"$_rh_p"
    ) 2>/dev/null || die 1 "cannot create $_rh_p"
  fi
  release_lock
  _rh_na=$(cert_enddate "$_rh_o/server.crt")
  SYLVODE_TLS_OUT=$_rh_o SYLVODE_TLS_CERT=$_rh_o/server.crt SYLVODE_TLS_KEY=$_rh_o/server.key \
    SYLVODE_TLS_FULLCHAIN=$_rh_o/fullchain.pem SYLVODE_TLS_NOT_AFTER=$_rh_na sh -c "$O_EXEC"
  _rh_rc=$?
  if [ "$_rh_rc" = 0 ]; then
    rm -f "$_rh_o/.reload-pending"
    return 0
  fi
  err "hook failed with status $_rh_rc (a reload is still pending)"
  exit 5
}

# print_server_block OUT THRESHOLD: stdout, the "Server" part of status.
print_server_block() {
  _ps_o=$1
  _ps_crt=$_ps_o/server.crt
  if [ ! -f "$_ps_crt" ]; then
    printf 'Server:    none\n'
    return 0
  fi
  printf 'Server:    %s\n' "$_ps_crt"
  if [ -f "$_ps_o/issue.conf" ]; then
    _ps_n=$(sed -n 's/^names=//p' "$_ps_o/issue.conf" | head -n 1 | tr -c 'A-Za-z0-9.:_ \n-' '?')
  else
    _ps_n="unknown (no issue.conf)"
  fi
  printf '  names    %s\n' "$_ps_n"
  printf '  serial   %s\n' "$(cert_serial "$_ps_crt")"
  printf '  until    %s\n' "$(cert_enddate "$_ps_crt")"
  if issuer_current "$_ps_crt"; then
    printf '  issuer   current CA\n'
  else
    printf '  issuer   NOT the current CA\n'
  fi
  if "$OPENSSL" x509 -in "$_ps_crt" -noout -checkend $(($2 * 86400)) >/dev/null 2>&1 && issuer_current "$_ps_crt"; then
    printf '  renewal  not due (threshold %s days)\n' "$2"
  else
    printf '  renewal  due\n'
  fi
  if [ -e "$_ps_o/.reload-pending" ]; then
    printf '  hook     reload pending\n'
  else
    printf '  hook     none pending\n'
  fi
}

print_ca_status() {
  printf 'CA:        %s\n' "$DIR/ca.crt"
  printf '  subject  %s\n' "$(cert_subject "$DIR/ca.crt")"
  printf '  names    %s\n' "$(join_list "$CA_NAMES" " ")"
  printf '  until    %s\n' "$(cert_enddate "$DIR/ca.crt")"
  printf '  sha256   %s\n' "$(cert_fp "$DIR/ca.crt")"
  if [ -f "$DIR/next.crt" ]; then
    printf '  pending  %s sha256 %s\n' "$DIR/next.crt" "$(cert_fp "$DIR/next.crt")"
  else
    printf '  pending  none\n'
  fi
}

# ---------------------------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------------------------

default_dir() {
  if [ -n "$O_DIR" ]; then
    DIR_RAW=$O_DIR
  elif [ -n "${SYLVODE_TLS_DIR:-}" ]; then
    DIR_RAW=$SYLVODE_TLS_DIR
  elif [ "$SELF_UID" = 0 ]; then
    DIR_RAW=/var/lib/sylvode-tls
  elif [ -n "${XDG_DATA_HOME:-}" ]; then
    DIR_RAW=$XDG_DATA_HOME/sylvode-tls
  elif [ -n "${HOME:-}" ]; then
    DIR_RAW=$HOME/.local/share/sylvode-tls
  else
    die 2 "cannot choose a state directory: pass --dir"
  fi
}

# ca_present: the CA files exist (else exit 4).
need_ca() {
  if [ ! -f "$DIR/ca.crt" ] || [ ! -f "$DIR/ca.names" ]; then
    die 4 "no CA in $DIR; run init-ca first"
  fi
}

cmd_init_ca() {
  [ -n "$NAMES" ] || usage_error "at least one --ip or --dns is required"
  _ic_days=${O_CA_DAYS:-3650}
  is_int "$_ic_days" 30 7300 || usage_error "--ca-days must be an integer from 30 to 7300"
  if [ -n "$O_CA_NAME" ]; then
    _ic_cn=$O_CA_NAME
    case $_ic_cn in
      *[!A-Za-z0-9\ ._-]*) usage_error "--ca-name may contain only letters, digits, space, \".\", \"_\" and \"-\"" ;;
    esac
    if [ "${#_ic_cn}" -lt 1 ] || [ "${#_ic_cn}" -gt 64 ]; then
      usage_error "--ca-name must be 1-64 characters"
    fi
  fi
  need_openssl3
  default_dir
  secure_dir "$DIR_RAW" 1
  DIR=$SECURED
  acquire_lock
  _ic_have_key=0
  _ic_have_crt=0
  [ -e "$DIR/ca/ca.key" ] && _ic_have_key=1
  [ -e "$DIR/ca.crt" ] && _ic_have_crt=1
  if [ "$_ic_have_key" = 1 ] && [ "$_ic_have_crt" = 1 ]; then
    [ -f "$DIR/ca.names" ] || die 3 "inconsistent CA state in $DIR: ca.names is missing; move $DIR away to start again"
    load_ca_names
    if list_equal "$CA_NAMES" "$NAMES"; then
      info "the CA already exists; nothing to do"
      print_ca_status
      return 0
    fi
    die 3 "a CA for $(join_list "$CA_NAMES" " ") already exists; use rotate-ca to change names"
  fi
  if [ "$_ic_have_key" = 1 ] || [ "$_ic_have_crt" = 1 ]; then
    die 3 "inconsistent CA state in $DIR: only one of ca/ca.key and ca.crt exists; move $DIR away to start again"
  fi
  if [ -z "$O_CA_NAME" ]; then
    _ic_cn="Sylvode intranet CA $(date -u +%Y-%m-%d) $("$OPENSSL" rand -hex 4)"
  fi
  make_ca "$DIR/ca" "$DIR/ca.crt" "$DIR/ca.names" "$NAMES" "$_ic_cn" "$_ic_days"
  print_ca_block "$DIR/ca.crt" "$NAMES" "$DIR/ca"
}

cmd_issue() {
  _ci_days=${O_DAYS:-397}
  is_int "$_ci_days" 1 825 || usage_error "--days must be an integer from 1 to 825"
  [ -z "$O_OWNER" ] || [ "$SELF_UID" = 0 ] || die 4 "--owner needs root"
  need_openssl3
  default_dir
  secure_dir "$DIR_RAW" 0
  DIR=$SECURED
  need_ca
  [ -f "$DIR/ca/ca.key" ] || die 4 "the CA key $DIR/ca/ca.key is missing"
  "$OPENSSL" x509 -in "$DIR/ca.crt" -noout -checkend 0 >/dev/null 2>&1 || die 3 "the CA has expired; run rotate-ca"
  secure_dir "${O_OUT:-$DIR/server}" 1
  _ci_out=$SECURED
  check_key_perms "$DIR/ca"
  load_ca_names
  parse_owner "$O_OWNER"
  acquire_lock
  _ci_names=$NAMES
  [ -n "$_ci_names" ] || _ci_names=$CA_NAMES
  list_subset "$_ci_names" "$CA_NAMES" || {
    printf '%s\n' "$_ci_names" | while IFS= read -r _ci_i; do
      [ -z "$_ci_i" ] || list_has "$CA_NAMES" "$_ci_i" || err "$_ci_i is not permitted by the CA"
    done
    exit 3
  }
  REQ_DAYS=$_ci_days
  if [ -e "$_ci_out/server.crt" ] && [ "$O_FORCE" != 1 ]; then
    if [ ! -f "$_ci_out/issue.conf" ]; then
      die 3 "$_ci_out holds a certificate without issue.conf; pass --force to replace it"
    fi
    read_issue_conf "$_ci_out"
    if ! list_equal "$IC_NAMES" "$_ci_names"; then
      die 3 "$_ci_out holds a certificate for $(join_list "$IC_NAMES" " "); pass --force to replace it"
    fi
    if ! renewal_due "$_ci_out" 60; then
      info "a matching valid certificate exists; nothing to do (use --force to reissue)"
      print_server_block "$_ci_out" 60
      return 0
    fi
    info "the existing certificate needs renewal: $DUE_REASON"
  fi
  issue_cert "$_ci_names" "$_ci_out" "$_ci_days"
  print_server_block "$_ci_out" 60
  if [ -n "$O_EXEC" ]; then
    run_hook "$_ci_out"
  fi
}

cmd_renew() {
  _cr_thr=${O_THRESHOLD_DAYS:-60}
  is_int "$_cr_thr" 1 400 || usage_error "--threshold-days must be an integer from 1 to 400"
  if [ -n "$O_DAYS" ]; then
    is_int "$O_DAYS" 1 825 || usage_error "--days must be an integer from 1 to 825"
  fi
  [ -z "$O_OWNER" ] || [ "$SELF_UID" = 0 ] || die 4 "--owner needs root"
  need_openssl3
  default_dir
  if [ "$O_CHECK" = 1 ]; then
    plain_dir "$DIR_RAW"
    DIR=$SECURED
    plain_dir "${O_OUT:-$DIR/server}"
    _cr_out=$SECURED
  else
    secure_dir "$DIR_RAW" 0
    DIR=$SECURED
    secure_dir "${O_OUT:-$DIR/server}" 0
    _cr_out=$SECURED
  fi
  need_ca
  load_ca_names
  if [ ! -f "$_cr_out/server.crt" ] || [ ! -f "$_cr_out/server.key" ] || [ ! -f "$_cr_out/issue.conf" ]; then
    die 4 "nothing to renew in $_cr_out; run issue"
  fi
  read_issue_conf "$_cr_out"
  REQ_DAYS=${O_DAYS:-$IC_DAYS}
  if [ -n "$O_DAYS" ] && [ "$_cr_thr" -ge "$O_DAYS" ]; then
    usage_error "--threshold-days ($_cr_thr) must be lower than --days ($O_DAYS)"
  fi
  if [ "$_cr_thr" -ge "$REQ_DAYS" ]; then
    warn "the threshold ($_cr_thr days) is not lower than the certificate lifetime ($REQ_DAYS days): every renewed certificate is due at once"
  fi
  KEY_UNREADABLE=0
  _cr_due=0
  if renewal_due "$_cr_out" "$_cr_thr"; then
    _cr_due=1
  fi
  if [ "$KEY_UNREADABLE" = 1 ]; then
    warn "$_cr_out/server.key is not readable: the key match was not checked"
  fi
  if [ "$O_CHECK" = 1 ]; then
    if [ "$_cr_due" = 1 ]; then
      info "renewal is due: $DUE_REASON"
      exit 10
    fi
    info "not due: $_cr_out/server.crt valid until $(cert_enddate "$_cr_out/server.crt"), threshold $_cr_thr days"
    return 0
  fi
  if [ "$_cr_due" = 1 ] || [ "$O_FORCE" = 1 ]; then
    if [ "$_cr_due" = 1 ]; then
      info "renewing: $DUE_REASON"
    else
      info "renewing: forced"
    fi
    [ -f "$DIR/ca/ca.key" ] || die 4 "the CA key $DIR/ca/ca.key is missing"
    "$OPENSSL" x509 -in "$DIR/ca.crt" -noout -checkend 0 >/dev/null 2>&1 || die 3 "the CA has expired; run rotate-ca"
    check_key_perms "$DIR/ca"
    _cr_owner=${O_OWNER:-$IC_OWNER}
    parse_owner "$_cr_owner"
    acquire_lock
    _cr_names=
    while IFS= read -r _cr_i; do
      [ -n "$_cr_i" ] || continue
      if list_has "$CA_NAMES" "$_cr_i"; then
        _cr_names=${_cr_names}${_cr_i}${NL}
      else
        warn "dropping $_cr_i: the CA no longer permits it"
      fi
    done <<EOF
$IC_NAMES
EOF
    [ -n "$_cr_names" ] || die 3 "none of the names of the certificate is permitted by the CA any more; run issue with new names"
    issue_cert "$_cr_names" "$_cr_out" "$REQ_DAYS"
    if [ -n "$O_EXEC" ]; then
      run_hook "$_cr_out"
    fi
    return 0
  fi
  info "not due: $_cr_out/server.crt valid until $(cert_enddate "$_cr_out/server.crt"), threshold $_cr_thr days"
  if [ -e "$_cr_out/.reload-pending" ]; then
    if [ -n "$O_EXEC" ]; then
      info "running the pending reload hook"
      run_hook "$_cr_out"
    else
      warn "a reload hook is pending ($_cr_out/.reload-pending); run renew with --exec"
    fi
  fi
  return 0
}

cmd_rotate_ca() {
  if [ "$O_PROMOTE" = 1 ] && [ "$O_ABORT" = 1 ]; then
    usage_error "--promote and --abort exclude each other"
  fi
  if { [ "$O_PROMOTE" = 1 ] || [ "$O_ABORT" = 1 ]; } && { [ -n "$NAMES" ] || [ -n "$O_CA_DAYS" ] || [ -n "$O_CA_NAME" ]; }; then
    usage_error "--promote and --abort take no names and no CA options"
  fi
  _cv_days=${O_CA_DAYS:-3650}
  is_int "$_cv_days" 30 7300 || usage_error "--ca-days must be an integer from 30 to 7300"
  if [ -n "$O_CA_NAME" ]; then
    _cv_cn=$O_CA_NAME
    case $_cv_cn in
      *[!A-Za-z0-9\ ._-]*) usage_error "--ca-name may contain only letters, digits, space, \".\", \"_\" and \"-\"" ;;
    esac
    if [ "${#_cv_cn}" -lt 1 ] || [ "${#_cv_cn}" -gt 64 ]; then
      usage_error "--ca-name must be 1-64 characters"
    fi
  fi
  need_openssl3
  default_dir
  if [ "$O_ABORT" = 1 ]; then
    resolve_dir "$DIR_RAW"
    if [ -n "$RES_TAIL" ]; then
      info "nothing is pending"
      return 0
    fi
  fi
  secure_dir "$DIR_RAW" 0
  DIR=$SECURED
  need_ca
  acquire_lock
  _cv_pending=0
  if [ -e "$DIR/next" ] || [ -e "$DIR/next.crt" ] || [ -e "$DIR/next.names" ]; then
    _cv_pending=1
  fi
  if [ "$O_ABORT" = 1 ]; then
    if [ -f "$DIR/next.crt" ] && [ ! -f "$DIR/next/ca.key" ]; then
      die 3 "refusing --abort: $DIR/next.crt exists but $DIR/next/ca.key does not, so a promote was interrupted and next.crt may be the only certificate for the installed CA key; run rotate-ca --promote to finish it"
    fi
    rm -rf "${DIR:?}/next" "${DIR:?}/next.crt" "${DIR:?}/next.names"
    info "the pending CA was discarded"
    return 0
  fi
  if [ "$O_PROMOTE" = 1 ]; then
    [ "$_cv_pending" = 1 ] || die 4 "no pending CA; run rotate-ca first"
    if [ ! -f "$DIR/next.crt" ] && [ ! -f "$DIR/next.names" ] && [ ! -f "$DIR/next/ca.key" ]; then
      rm -rf "${DIR:?}/next"
      info "the promotion was already complete"
      return 0
    fi
    # An interrupted promotion has already moved the new key: finish it.
    _cv_resume=0
    if [ -f "$DIR/next.crt" ] && [ ! -f "$DIR/next/ca.key" ] && [ -f "$DIR/ca/ca.key" ] && key_matches "$DIR/ca/ca.key" "$DIR/next.crt"; then
      _cv_resume=1
      warn "resuming an interrupted promotion"
    fi
    if [ "$_cv_resume" = 0 ]; then
      if [ ! -f "$DIR/next.crt" ] || [ ! -f "$DIR/next.names" ] || [ ! -f "$DIR/next/ca.key" ]; then
        die 3 "inconsistent pending CA in $DIR; run rotate-ca --abort and start again"
      fi
      check_key_perms "$DIR/next"
    fi
    [ -f "$DIR/ca/ca.key" ] || die 3 "the current CA key $DIR/ca/ca.key is missing"
    check_key_perms "$DIR/ca"
    _cv_fp=$(cert_fp "$DIR/ca.crt")
    _cv_name=$(date -u +%Y%m%dT%H%M%SZ)-$(fp16 "$_cv_fp").crt
    if [ ! -d "$DIR/retired" ]; then
      mkdir -m 0755 "$DIR/retired" || die 1 "cannot create $DIR/retired"
    fi
    mk_tmp "$DIR/retired"
    cp "$DIR/ca.crt" "$TMPD/retired.crt" || die 1 "cannot copy the old CA certificate"
    chmod 0644 "$TMPD/retired.crt"
    mv -f "$TMPD/retired.crt" "$DIR/retired/$_cv_name" || die 1 "cannot store the old CA certificate"
    rm -rf "$TMPD"
    if [ -f "$DIR/next/ca.key" ]; then
      mv -f "$DIR/next/ca.key" "$DIR/ca/ca.key" || die 1 "cannot install the new CA key"
    fi
    if [ -f "$DIR/next.names" ]; then
      mv -f "$DIR/next.names" "$DIR/ca.names" || die 1 "cannot install the new name list"
    fi
    mv -f "$DIR/next.crt" "$DIR/ca.crt" || die 1 "cannot install the new CA certificate"
    rm -rf "${DIR:?}/next"
    info "the new CA is current; the old key was deleted"
    printf 'Retired CA certificate: %s\n' "$DIR/retired/$_cv_name"
    printf 'Leaves signed by the old CA are reissued by the next renew run (or run renew now).\n'
    printf 'Remove the old CA from clients with trust-client remove %s.\n' "$DIR/retired/$_cv_name"
    return 0
  fi
  [ "$_cv_pending" = 0 ] || die 3 "a rotation is already pending; run rotate-ca --promote or --abort"
  load_ca_names
  _cv_names=$NAMES
  [ -n "$_cv_names" ] || _cv_names=$CA_NAMES
  if [ -z "$O_CA_NAME" ]; then
    _cv_cn="Sylvode intranet CA $(date -u +%Y-%m-%d) $("$OPENSSL" rand -hex 4)"
  fi
  make_ca "$DIR/next" "$DIR/next.crt" "$DIR/next.names" "$_cv_names" "$_cv_cn" "$_cv_days"
  print_ca_block "$DIR/next.crt" "$_cv_names" "$DIR/next"
  printf 'Install %s on every client, then run rotate-ca --promote.\n' "$DIR/next.crt"
}

cmd_fingerprint() {
  need_openssl_read
  default_dir
  plain_dir "$DIR_RAW"
  DIR=$SECURED
  if [ -n "$O_CERT" ]; then
    [ -f "$O_CERT" ] || die 2 "not a file: $(safe_str "$O_CERT")"
    _cf_fp=$(cert_fp "$O_CERT")
    [ -n "$_cf_fp" ] || die 2 "not a PEM certificate: $(safe_str "$O_CERT")"
    printf 'SHA-256 %s  %s\n' "$_cf_fp" "$O_CERT"
    return 0
  fi
  [ -f "$DIR/ca.crt" ] || die 4 "no CA in $DIR; run init-ca first"
  _cf_fp=$(cert_fp "$DIR/ca.crt")
  [ -n "$_cf_fp" ] || die 2 "not a PEM certificate: $DIR/ca.crt"
  printf 'SHA-256 %s  %s\n' "$_cf_fp" "$DIR/ca.crt"
  if [ -f "$DIR/next.crt" ]; then
    _cf_fp=$(cert_fp "$DIR/next.crt")
    [ -n "$_cf_fp" ] || die 2 "not a PEM certificate: $DIR/next.crt"
    printf 'SHA-256 %s  %s\n' "$_cf_fp" "$DIR/next.crt"
  fi
}

cmd_status() {
  _st_thr=${O_THRESHOLD_DAYS:-60}
  is_int "$_st_thr" 1 400 || usage_error "--threshold-days must be an integer from 1 to 400"
  need_openssl_read
  default_dir
  plain_dir "$DIR_RAW"
  DIR=$SECURED
  need_ca
  load_ca_names
  plain_dir "${O_OUT:-$DIR/server}"
  print_ca_status
  print_server_block "$SECURED" "$_st_thr"
}

# host_of ITEM: "IP:x" -> x (IPv6 in brackets), "DNS:y" -> y.
host_of() {
  case $1 in
    IP:*:*) printf '[%s]\n' "${1#IP:}" ;;
    IP:*) printf '%s\n' "${1#IP:}" ;;
    DNS:*) printf '%s\n' "${1#DNS:}" ;;
  esac
}

cmd_config_hint() {
  _ch_port=${O_PORT:-443}
  is_int "$_ch_port" 1 65535 || usage_error "--port must be an integer from 1 to 65535"
  _ch_hport=${O_HTTP_PORT:-80}
  is_int "$_ch_hport" 0 65535 || usage_error "--http-port must be an integer from 0 to 65535"
  _ch_up=${O_UPSTREAM:-127.0.0.1:3000}
  case $_ch_up in
    *[!A-Za-z0-9.:\[\]-]* | "" | *:) usage_error "--upstream must be HOST:PORT" ;;
  esac
  is_int "${_ch_up##*:}" 1 65535 || usage_error "--upstream must be HOST:PORT"
  _ch_proxy=${O_PROXY:-all}
  case $_ch_proxy in
    nginx | caddy | all) ;;
    *) usage_error "--proxy must be nginx, caddy or all" ;;
  esac
  for _ch_p in "$O_CERT_PATH" "$O_KEY_PATH"; do
    case $_ch_p in
      "") ;;
      /*[!A-Za-z0-9/._@+:-]* | [!/]*) usage_error "--cert-path and --key-path must be absolute paths of letters, digits and \"/._@+:-\"" ;;
    esac
  done
  need_openssl3
  default_dir
  plain_dir "$DIR_RAW"
  DIR=$SECURED
  need_ca
  load_ca_names
  plain_dir "${O_OUT:-$DIR/server}"
  _ch_out=$SECURED
  _ch_cert=${O_CERT_PATH:-$_ch_out/fullchain.pem}
  _ch_key=${O_KEY_PATH:-$_ch_out/server.key}
  case $_ch_cert$_ch_key in
    *[!A-Za-z0-9/._@+:-]*) usage_error "the certificate and key paths must consist of letters, digits and \"/._@+:-\"" ;;
  esac
  _ch_list=$CA_NAMES
  if [ -f "$_ch_out/issue.conf" ]; then
    read_issue_conf "$_ch_out"
    _ch_list=$IC_NAMES
  fi
  _ch_first=$(printf '%s\n' "$CA_NAMES" | head -n 1)
  if [ -n "$O_NAME" ]; then
    case $O_NAME in
      *:*) _ch_n=ip ;;
      *[!0-9.]*) _ch_n=dns ;;
      *) _ch_n=ip ;;
    esac
    if [ "$_ch_n" = ip ]; then
      _ch_v=$(canon_ip "$O_NAME") || exit 2
      _ch_first=IP:$_ch_v
    else
      _ch_v=$(canon_dns "$O_NAME") || exit 2
      _ch_first=DNS:$_ch_v
    fi
  fi
  _ch_primary=$(host_of "$_ch_first")
  _ch_hosts=
  _ch_origins=
  _ch_caddy=
  _ch_all=$_ch_first$NL$_ch_list
  _ch_seen=
  while IFS= read -r _ch_i; do
    [ -n "$_ch_i" ] || continue
    list_has "$_ch_seen" "$_ch_i" && continue
    _ch_seen=${_ch_seen}${_ch_i}${NL}
    _ch_h=$(host_of "$_ch_i")
    if [ "$_ch_port" = 443 ]; then
      _ch_o=https://$_ch_h
    else
      _ch_o=https://$_ch_h:$_ch_port
    fi
    _ch_hosts=${_ch_hosts:+$_ch_hosts }$_ch_h
    _ch_origins=${_ch_origins:+$_ch_origins, }\"$_ch_o\"
    _ch_caddy=${_ch_caddy:+$_ch_caddy, }$_ch_o
  done <<EOF
$_ch_all
EOF
  if [ "$_ch_port" = 443 ]; then
    _ch_primary_origin=https://$_ch_primary
  else
    _ch_primary_origin=https://$_ch_primary:$_ch_port
  fi
  _ch_fp=$(cert_fp "$DIR/ca.crt")
  cat <<EOF
# ---- Sylvode configuration (sylvode.toml) for $_ch_primary_origin ----
[server]
public_url = "$_ch_primary_origin"

[flow]
collab_allowed_origins = [$_ch_origins]

# Remove this line from [auth] if present: over HTTPS the auth cookies must carry Secure.
#   allow_insecure_cookies = true
# Only when the OAuth authorization server is enabled:
#   [oauth]
#   issuer = "$_ch_primary_origin"
#   [mcp]
#   oauth_issuer = "$_ch_primary_origin"
#   oauth_resource = "<the exact https URL that receives MCP JSON-RPC posts>"
# If the proxy runs on another host than the web UI, list its address in [server] trusted_proxies.
#
# Clients:
#   runner config:        api_url = "$_ch_primary_origin"
#   sylvode / mcp-server: --api-url $_ch_primary_origin
#   MCP over http (only if the proxy forwards /mcp/ to the MCP server): $_ch_primary_origin/mcp/rpc
# Clients must trust the CA first (trust-client.sh / trust-client.ps1), SHA-256:
#   $_ch_fp
EOF
  if [ "$_ch_proxy" = nginx ] || [ "$_ch_proxy" = all ]; then
    cat <<EOF

# ---- nginx (inside the http {} context, for example /etc/nginx/conf.d/sylvode-tls.conf) ----
map \$http_upgrade \$sylvode_tls_connection_upgrade {
    default upgrade;
    ''      close;
}
map \$args \$sylvode_tls_loggable_args {
    ~*(^|&)(ticket|signature)=  "redacted";
    default                     \$args;
}
log_format sylvode_tls_redacted '\$remote_addr - \$remote_user [\$time_local] '
                                '"\$request_method \$uri\$is_args\$sylvode_tls_loggable_args \$server_protocol" '
                                '\$status \$body_bytes_sent "\$http_referer" "\$http_user_agent"';
server {
    listen $_ch_port ssl;
    http2 on;                      # nginx >= 1.25.1; older: "listen $_ch_port ssl http2;" and drop this line
    server_name $_ch_hosts;
    ssl_certificate     $_ch_cert;
    ssl_certificate_key $_ch_key;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_session_cache shared:SylvodeTLS:1m;
    ssl_session_timeout 1d;
    ssl_session_tickets off;
    client_max_body_size 200m;
    access_log /var/log/nginx/sylvode-tls.access.log sylvode_tls_redacted;
    # Browsers ignore HSTS for IP-address hosts; it takes effect for DNS names.
    add_header Strict-Transport-Security "max-age=31536000" always;
    location / {
        proxy_pass http://$_ch_up;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection \$sylvode_tls_connection_upgrade;
        proxy_buffering off;       # the OAuth event stream must not be buffered
        proxy_read_timeout 1h;     # live editing holds a WebSocket open
        proxy_send_timeout 1h;
    }
}
EOF
    if [ "$_ch_hport" != 0 ]; then
      cat <<EOF
server {
    listen $_ch_hport;
    server_name $_ch_hosts;
    return 308 $_ch_primary_origin\$request_uri;
}
EOF
    fi
    cat <<EOF
# Reload after renewal: sylvode-tls.sh renew --exec "systemctl reload nginx"
EOF
  fi
  if [ "$_ch_proxy" = caddy ] || [ "$_ch_proxy" = all ]; then
    cat <<EOF

# ---- Caddy (Caddyfile) ----
$_ch_caddy {
	tls $_ch_cert $_ch_key
	header Strict-Transport-Security "max-age=31536000"
	reverse_proxy $_ch_up
	log {
		output stdout
		format filter {
			wrap json
			fields {
				request>uri query {
					delete ticket
					delete signature
				}
			}
		}
	}
}
# Caddy redirects http:// to https:// by itself. Issue with --owner caddy:caddy so it can read the key.
# Reload after renewal: sylvode-tls.sh renew --exec "caddy reload --config /etc/caddy/Caddyfile --force"
EOF
  fi
}

# ---------------------------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------------------------

if [ $# -lt 1 ]; then
  usage_main >&2
  exit 2
fi
case $1 in
  --help | -h)
    usage_main
    exit 0
    ;;
  --version)
    printf '%s %s\n' "$PROG" "$VERSION"
    exit 0
    ;;
  init-ca | issue | renew | rotate-ca | fingerprint | status | config-hint) ;;
  *)
    err "unknown subcommand: $(safe_str "$1")"
    usage_main >&2
    exit 2
    ;;
esac
SUB=$1
shift

VALOPTS="dir openssl"
FLAGOPTS="quiet"
case $SUB in
  init-ca) VALOPTS="$VALOPTS ip dns ca-days ca-name" ;;
  issue)
    VALOPTS="$VALOPTS ip dns out owner days exec"
    FLAGOPTS="$FLAGOPTS force"
    ;;
  renew)
    VALOPTS="$VALOPTS out threshold-days days owner exec"
    FLAGOPTS="$FLAGOPTS force check"
    ;;
  rotate-ca)
    VALOPTS="$VALOPTS ip dns ca-days ca-name"
    FLAGOPTS="$FLAGOPTS promote abort"
    ;;
  fingerprint) VALOPTS="$VALOPTS cert" ;;
  status) VALOPTS="$VALOPTS out threshold-days" ;;
  config-hint) VALOPTS="$VALOPTS out name port http-port upstream proxy cert-path key-path" ;;
esac

parse_opts "$@"

case $SUB in
  init-ca) cmd_init_ca ;;
  issue) cmd_issue ;;
  renew) cmd_renew ;;
  rotate-ca) cmd_rotate_ca ;;
  fingerprint) cmd_fingerprint ;;
  status) cmd_status ;;
  config-hint) cmd_config_hint ;;
esac
