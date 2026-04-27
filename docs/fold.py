"""
Fold the Trp-cage miniprotein with ColabFold via bv.

Usage:
    python3 fold.py

Requires:
    bv add colabfold   (done once per machine)
"""

import json
import os
import subprocess
import sys
from pathlib import Path

FASTA_CONTENT = """>trp-cage
NLYIQWLKDGGPSSGRPPPS
"""

FASTA_PATH = Path("trpcage.fasta")
OUTPUT_DIR = Path("output")


def write_fasta() -> None:
    FASTA_PATH.write_text(FASTA_CONTENT)


def run_colabfold() -> None:
    OUTPUT_DIR.mkdir(exist_ok=True)
    cmd = [
        "bv", "run", "colabfold", "--",
        "--num-recycle", "3",
        f"/workspace/{FASTA_PATH}",
        f"/workspace/{OUTPUT_DIR}",
    ]
    print(f"Running ColabFold on trp-cage (20 aa)...")
    print(f"Output directory: {OUTPUT_DIR}/\n")
    result = subprocess.run(cmd, check=False)
    if result.returncode != 0:
        print("ColabFold run failed.", file=sys.stderr)
        sys.exit(result.returncode)


def print_results() -> None:
    pdbs = sorted(OUTPUT_DIR.glob("*.pdb"))
    score_files = sorted(OUTPUT_DIR.glob("*scores*.json"))

    if not pdbs:
        print("No PDB output found in output/", file=sys.stderr)
        sys.exit(1)

    print("Results:")
    for p in pdbs[:3]:
        print(f"  {p.name}")
    for s in score_files[:3]:
        print(f"  {s.name}")

    # pLDDT from the top-ranked scores JSON
    top_scores = score_files[0] if score_files else None
    if top_scores:
        data = json.loads(top_scores.read_text())
        plddt = data.get("plddt", [])
        sequence = FASTA_CONTENT.strip().splitlines()[1]
        print("\npLDDT scores (per residue):")
        for aa, score in zip(sequence, plddt):
            print(f"  {aa}   {score:.1f}")
        if plddt:
            mean = sum(plddt) / len(plddt)
            print(f"\nMean pLDDT: {mean:.1f}  (> 70 is considered confident)")

    print(f"\nTop structure written to: {OUTPUT_DIR}/{pdbs[0].name}")


def main() -> None:
    os.chdir(Path(__file__).parent)
    write_fasta()
    run_colabfold()
    print_results()


if __name__ == "__main__":
    main()
