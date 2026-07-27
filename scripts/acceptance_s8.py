#!/usr/bin/env python3
"""STS-X 3.0 §8.1 acceptance measurement.

For three §1 queries on retouch_app, measure:
  - --locate  : JSON token count, assert <= 200 tok
  - --expand  : JSON (code) token count vs grep+Read-full-file baseline,
                 assert >= 80% savings

Usage: python3 acceptance_s8.py <STSX_BIN> <PROJECT_ROOT>
"""
import sys, json, subprocess, tiktoken

ENC = tiktoken.get_encoding("cl100k_base")

def tok(s: str) -> int:
    return len(ENC.encode(s))

def run(bin_path, root, args):
    r = subprocess.run([bin_path, *args], capture_output=True, text=True, cwd=root)
    if r.returncode != 0:
        sys.stderr.write(f"[stderr] {r.stderr}\n")
        raise RuntimeError(f"cmd failed: {args}")
    return r.stdout

def expand_baseline_tokens(root, query):
    # grep -rln across code files -> read each full file (what an AI does w/o sts-x)
    r = subprocess.run(
        ["grep", "-rln", "--include=*.py", "--include=*.rs", "--include=*.js",
         "--include=*.ts", "--include=*.go", "--include=*.java", query, root],
        capture_output=True, text=True)
    files = [f for f in r.stdout.splitlines() if f]
    total = 0
    for f in files[:5]:  # conservative: cap to top 5 files
        try:
            with open(f, "r", errors="ignore") as fh:
                total += tok(fh.read())
        except Exception:
            pass
    return max(total, 1)

QUERIES = ["select_best_cfg", "class QwenVLClient", "def decide"]

def main():
    bin_path = sys.argv[1] if len(sys.argv) > 1 else "/Users/xtap/Documents/AI/sts-x/target/release/sts-x"
    root = sys.argv[2] if len(sys.argv) > 2 else "/Users/xtap/Documents/AI/retouch_app"

    print("=" * 70)
    print("STS-X 3.0  §8.1 acceptance  (project: %s)" % root)
    print("=" * 70)

    all_ok = True
    for q in QUERIES:
        print(f"\n### query: {q!r}")
        # --- locate ---
        loc_json = run(bin_path, root, ["search", q, "--locate", "-t", "1"])
        loc_tok = tok(loc_json)
        loc_ok = loc_tok <= 200
        all_ok &= loc_ok
        print(f"  locate  : {loc_tok:4d} tok  [<=200]  {'PASS' if loc_ok else 'FAIL'}")
        # print sample
        try:
            print("    -> " + json.dumps(json.loads(loc_json), ensure_ascii=False)[:160])
        except Exception:
            pass

        # --- expand ---
        exp_json = run(bin_path, root, ["search", q, "--expand", "-t", "3"])
        exp_obj = json.loads(exp_json)
        exp_code = "\n".join(it["code"] for it in exp_obj.get("results", []))
        exp_tok = tok(exp_code)
        base_tok = expand_baseline_tokens(root, q)
        savings = 1.0 - (exp_tok / base_tok)
        exp_ok = savings >= 0.80
        all_ok &= exp_ok
        print(f"  expand  : {exp_tok:4d} tok   (baseline grep+Read full file = {base_tok} tok)")
        print(f"           savings = {savings*100:5.1f}%  [>=80%]  {'PASS' if exp_ok else 'FAIL'}")
        if not exp_ok:
            print(f"    NOTE: baseline may be small if grep hits few files; check the symbol.")

    print("\n" + "=" * 70)
    print("RESULT:", "ALL PASS ✅" if all_ok else "SOME FAIL ❌")
    print("=" * 70)
    sys.exit(0 if all_ok else 1)

if __name__ == "__main__":
    main()
