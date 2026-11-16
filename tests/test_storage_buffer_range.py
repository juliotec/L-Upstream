"""The decoders' storage buffer ranges stay within maxStorageBufferRange.

upload_astc_gpu and upload_unswizzle_gpu bind the staged texture as a storage buffer from a level's offset to the end of
the staging. That range grows with the texture's layers and is not bounded by anything else, while a device only has
to take maxStorageBufferRange (at least 2^27 bytes): VUID-VkWriteDescriptorSet-descriptorType-00333. Each range is
clamped to the limit (the shaders read one level, which lies at the start of the range).

python -I -B tests/test_storage_buffer_range.py [--source <checkout>]
"""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
LIMIT = "dev.props.limits.maxStorageBufferRange"


def main():
	p = argparse.ArgumentParser(description=__doc__)
	p.add_argument("--source", type=Path, default=ROOT)
	a = p.parse_args()
	tex = (a.source / "gpu/vk/vk_texture.cpp").read_text(encoding="utf-8")
	bad = []
	for fn in ("bool upload_astc_gpu(", "bool upload_unswizzle_gpu("):
		at = tex.index(fn)
		body = tex[at:tex.index("\n}\n", at)]
		infos = re.findall(r"VkDescriptorBufferInfo bi\{([^;]*)\};", body)
		if not infos:
			bad.append("%s: no storage buffer descriptor found: this test needs updating" % fn)
		for info in infos:
			if LIMIT not in info:
				bad.append("%s binds {%s} without clamping it to maxStorageBufferRange (VUID 00333)" % (fn, info))
	if bad:
		raise SystemExit("\n".join(bad))
	print("the decoders' storage buffer ranges are within maxStorageBufferRange: ok")


if __name__ == "__main__":
	main()
