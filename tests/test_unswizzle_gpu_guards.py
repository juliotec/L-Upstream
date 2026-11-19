"""upload_unswizzle_gpu leaves sparse textures to the CPU path, and marks the bound descriptors disturbed as soon as it binds.

gpu/vk/vk_texture.cpp upload_unswizzle_gpu unswizzles a texture with a compute shader that stores into a storage view
of its image. A sparse image has no memory in a block until sparse_flush binds it, and a store into an unbound block
is dropped: those textures must go through the CPU path, which binds their blocks. The function also binds unsw_pipe
and its push descriptors on the upload command buffer: when a level's view then fails it returns false, the CPU path
takes over, and the draw state is told its compute bindings are gone right after the bind (descriptors_disturbed, defensive: the
descriptor cache forgets whatever was pushed there), not only at the end. This reads the function from the source and checks both.

python -I -B tests/test_unswizzle_gpu_guards.py [--source <checkout>]
"""
import argparse
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=ROOT)
    a = p.parse_args()
    body = block((a.source / "gpu/vk/vk_texture.cpp").read_text(encoding="utf-8"), "bool upload_unswizzle_gpu(")
    bad = []
    first = body[:body.index("return false;")]
    if not re.search(r"\btex\.sparse\b", first):
        bad.append("a sparse texture is unswizzled on the GPU into blocks no memory is bound to")
    bind = body.index("vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, unsw_pipe);")
    after = body[bind:]
    early = after.find("return false;")
    disturbed = after.find("descriptors_disturbed();")
    if disturbed < 0 or (early >= 0 and disturbed > early):
        bad.append("a failed view returns false after unsw_pipe is bound without descriptors_disturbed()")
    if bad:
        raise SystemExit("\n".join(bad))
    print("GPU unswizzle: sparse textures left to the CPU path, descriptors disturbed at the bind: ok")


if __name__ == "__main__":
    main()
