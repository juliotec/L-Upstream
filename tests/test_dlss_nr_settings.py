"""DLSS 5's model settings: edits coalesced before the feature is made again, their bounds, and the model's working size
(gpu/vk/dlss_nr_settings.h), without NVIDIA's runtime."""
RUN_ALL_ARGS = ["--cxx", "{cxx}"]

import argparse
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))

import subprocess
import tempfile

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--cxx', default='g++')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='nr-settings-') as tmp:
        exe = Path(tmp) / 'nr-settings.exe'
        subprocess.run([args.cxx, '-std=c++20', '-Wall', '-Wextra', '-Werror',
                        str(Path(__file__).with_suffix('.cpp')), '-o', str(exe)], check=True)
        subprocess.run([str(exe)], check=True)

if __name__ == '__main__':
    main()
