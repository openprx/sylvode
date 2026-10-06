#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd); EVIDENCE="$ROOT/.flow-gate/evidence/v1.0"; CONTRACTS="${SYLVODE_CONTRACTS_ROOT:-}"; RESULT=
[[ $# -eq 0 || $1 == --* ]] || { RESULT=$1; shift; }
while (($#)); do case "$1" in
 --repo-root) ROOT=${2:?};shift 2;; --evidence-root) EVIDENCE=${2:?};shift 2;; --contracts-root) CONTRACTS=${2:?};shift 2;; --json) shift;;
 --predecessor-gate-result|--predecessor-evidence) shift 2;; *) echo "FAIL: unsupported argument: $1" >&2;exit 2;; esac; done
[[ -n $CONTRACTS && -d $CONTRACTS ]] || { echo "FAIL: contracts checkout not found (${CONTRACTS:-unset}); pass --contracts-root DIR or set SYLVODE_CONTRACTS_ROOT" >&2; exit 2; }
[[ -n $RESULT ]] || RESULT="$EVIDENCE/gate-result.json"
python3 - "$RESULT" "$EVIDENCE" "$ROOT" "$CONTRACTS/gates/v1.0-gate.yaml" <<'PY'
import hashlib,json,pathlib,re,subprocess,sys,yaml
result,evidence,repo,gate_path=map(pathlib.Path,sys.argv[1:]); drift=[]
def same(k,a,b):
 if a!=b:drift.append({"field":k,"observed":a,"expected":b})
try:receipt=json.loads(result.read_text());gate=yaml.safe_load(gate_path.read_text())
except Exception as e:print(json.dumps({"receipt_consistent":False,"errors":[str(e)]},sort_keys=True));raise SystemExit(2)
head=subprocess.check_output(["git","-C",str(repo),"rev-parse","HEAD"],text=True).strip();rust=re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"',(repo/"Cargo.toml").read_text(),re.S).group(1);frontend=json.loads((repo/"frontend/package.json").read_text())["version"]
dirty=subprocess.check_output(["git","-C",str(repo),"status","--porcelain=v1"],text=True).splitlines()
same("schema_version",receipt.get("schema_version"),"sylvode.flow.gate-result.v1");same("release",receipt.get("release"),"1.0.0");same("source.head",receipt.get("source",{}).get("head"),head);same("source.rust_workspace_version",receipt.get("source",{}).get("rust_workspace_version"),rust);same("source.frontend_package_version",receipt.get("source",{}).get("frontend_package_version"),frontend);same("source.dirty",receipt.get("source",{}).get("dirty"),bool(dirty));same("gate_contract.status",receipt.get("gate_contract",{}).get("status"),gate.get("status"));same("gate_contract.sha256",receipt.get("gate_contract",{}).get("sha256"),hashlib.sha256(gate_path.read_bytes()).hexdigest());same("hard_gates.keys",sorted(receipt.get("hard_gates",{})),sorted(gate.get("hard_gates",{})))
checks={r.get("id"):r for r in receipt.get("checks",[])}; violations=[]
for cid,row in checks.items():
 try:same("checks."+cid+".sha256",row.get("sha256"),hashlib.sha256((evidence/row["log"]).read_bytes()).hexdigest())
 except Exception as e:drift.append({"field":"checks."+cid+".log","error":str(e)})
 try:
  json.loads((evidence/row["artifact"]).read_text()); artifact_status="parsed"
 except Exception:artifact_status="missing_or_unparseable"
 same("checks."+cid+".artifact_status",row.get("artifact_status"),artifact_status)
 if artifact_status!="parsed" or int(row.get("executed_count",0))<=0:violations.append(cid)
same("producer_execution.violations",receipt.get("producer_execution",{}).get("violations"),violations)
for cid,key in (("brand_residue","brand_residue_result"),("compat_deprecation","compat_deprecation_result")):
 same("checks."+cid+".artifact",checks.get(cid,{}).get("artifact"),pathlib.Path(gate.get("artifacts",{}).get(key,"")).name or None)
hard=dict(receipt.get("hard_gates",{}));same("frontend_track_accepted",hard.get("frontend_track_accepted"),"pending")
# ADR-0020 gates, recomputed here from the artifacts the contract names (not from the receipt's
# check rows): the residue scan and the compatibility/deprecation run.
declared=gate.get("artifacts",{})
def artifact(key):
 try:return json.loads((evidence/pathlib.Path(declared[key]).name).read_text())
 except Exception as e:drift.append({"field":"artifacts."+key,"error":str(e)});return {}
def log_matches(entry):
 try:path=evidence/entry["log"];body=path.read_bytes()
 except Exception as e:drift.append({"field":"log."+str(entry.get("name")),"error":str(e)});return None
 same("log."+str(entry.get("name"))+".sha256",entry.get("sha256"),hashlib.sha256(body).hexdigest());return body.decode(errors="replace")
residue=artifact("brand_residue_result");expected_repos={"sylvode","openpr-webhook","docs","site",".github"}
repos=[r for r in residue.get("repos",[]) if isinstance(r,dict)];by_name={r.get("name"):r for r in repos}
uncovered=sum(len(r.get("uncovered") or []) for r in repos)
summary=residue.get("summary") or {}
# The per-repository hit lists and the summary must both say zero; an artifact whose two disagree fails.
residue_passed=(residue.get("passed") is True and residue.get("release") is True and residue.get("strict") is True and uncovered==0 and summary.get("uncovered")==0 and summary.get("stale_entries")==0
 and not residue.get("stale_entries") and not residue.get("missing_expected_repos") and len(repos)==len(expected_repos) and set(by_name)==expected_repos
 and all(r.get("reachable") is True and (r.get("files_scanned") or 0)>0 and r.get("dirty") is False and not r.get("errors") for r in repos)
 and by_name.get("sylvode",{}).get("head")==head and residue.get("source_head")==head)
compat=artifact("compat_deprecation_result");compat_checks=[c for c in compat.get("checks",[]) if isinstance(c,dict)];compat_mutations=[m for m in compat.get("mutation_controls",[]) if isinstance(m,dict)]
def named_tests_ran(entry):
 body=log_matches(entry)
 if body is None:return False
 ran_ok=set(re.findall(r"^test (\S+) \.\.\. ok$",body,re.M))
 return bool(entry.get("required_tests")) and all(t in ran_ok for t in entry["required_tests"]) and not re.search(r"^test \S+ \.\.\. FAILED$",body,re.M)
def mutation_red(entry):
 body=log_matches(entry)
 return body is not None and entry.get("exit_code")!=0 and entry.get("seam_applied") is True and re.search(r"^test middleware::bot_auth::tests::attribution_accepts_either_spelling_and_refuses_disagreement \.\.\. FAILED$",body,re.M) is not None
names={c.get("name") for c in compat_checks};required_names={"adr0020-cli-nine-groups-stdout-byte-equal","adr0020-legacy-warns-canonical-silent","adr0020-attribution-conflict-rejected","adr0020-resource-alias-meta-deprecation"}
compat_passed=(compat.get("passed") is True and compat.get("release")=="1.0.0" and compat.get("source_head")==head and required_names<=names
 and all(c.get("status")=="passed" for c in compat_checks) and all(named_tests_ran(c) for c in compat_checks if c.get("name") in required_names)
 and bool(compat_mutations) and all(m.get("detected") is True for m in compat_mutations)
 and any(m.get("name")=="attribution-conflict-resolves-to-canonical" and mutation_red(m) for m in compat_mutations))
for key,value in (("brand_residue_release_gate",residue_passed),("openpr_compat_deprecation_verified",compat_passed)):
 if key in gate.get("hard_gates",{}):
  same("hard_gates."+key,hard.get(key),"passed" if value else "failed");hard[key]="passed" if value else "failed"
automated=[k for k in gate.get("hard_gates",{}) if k!="frontend_track_accepted"]
auto=all(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted") and not violations and not dirty
same("automated_ready",receipt.get("automated_ready"),auto);same("automated_gate_count",receipt.get("automated_gate_count"),len(automated));same("automated_passed",receipt.get("automated_passed"),sum(v=="passed" for k,v in hard.items() if k!="frontend_track_accepted"));same("automated_failed",receipt.get("automated_failed"),sum(v=="failed" for k,v in hard.items() if k!="frontend_track_accepted"))
baseline=gate["source_baseline"];baseline_ok=baseline.get("reviewed_head")==head and str(baseline.get("rust_workspace_version"))==rust and str(baseline.get("frontend_package_version"))==frontend
candidate=auto and gate.get("status")=="accepted" and baseline_ok and receipt.get("predecessor",{}).get("accepted") is True and hard.get("frontend_track_accepted")=="passed";accepted=candidate and all(v.get("status")=="passed" for v in receipt.get("manual_signoffs",{}).values())
same("candidate_ready",receipt.get("candidate_ready"),candidate);same("accepted",receipt.get("accepted"),accepted)
out={"schema_version":"sylvode.flow.verification.v2","release":"1.0.0","receipt_consistent":not drift,"drift":drift,"hard_gates":hard,"automated_ready":auto,"candidate_ready":candidate,"accepted":accepted,"producer_violations":violations,"blockers":receipt.get("blockers",[])}
print(json.dumps(out,sort_keys=True));raise SystemExit(0 if not drift and auto else 1)
PY
