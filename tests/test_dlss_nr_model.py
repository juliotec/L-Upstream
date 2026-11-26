"""Which file runs DLSS 5 (gpu/vk/dlss_nr_model.h), with no GPU, NGX or file: the choice table for every GPU, file pair and
DLSS 5 model row choice (an RTX 50 with both files: the row's; else the FP16 file when it is there, else NVIDIA's), the
row shown only on an RTX 50 with both, SHA-256 against FIPS 180-4's examples (split anyhow), and the FP16 allow-list.

    python -I -B tests/test_dlss_nr_model.py [--cxx g++]
"""
RUN_ALL_ARGS = ["--cxx", "{cxx}"]

import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="nr-model-") as tmp:
        exe = Path(tmp) / "nr-model.exe"
        subprocess.run([args.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", str(Path(__file__).with_suffix(".cpp")),
                        "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
