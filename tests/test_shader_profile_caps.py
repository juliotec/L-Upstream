"""Check that the shader profile follows the GPU's float controls, 64-bit atomics and subgroup stages, without a GPU.

make_profile (gpu/vk/vk_shader.cpp) tells the translator what the GPU's SPIR-V may use. Each execution mode, atomic and
subgroup operation it allows is invalid SPIR-V on a GPU without it: SignedZeroInfNanPreserve 32 on one whose
shaderSignedZeroInfNanPreserveFloat32 is false (VUID-RuntimeSpirv-shaderSignedZeroInfNanPreserveFloat32-06294),
DenormPreserve or DenormFlushToZero 32 without shaderDenormPreserveFloat32 or shaderDenormFlushToZeroFloat32 (06297,
06300), a 64-bit atomic on a storage buffer without shaderBufferInt64Atomics (VUID-RuntimeSpirv-None-06278) and a
subgroup operation in a stage outside subgroupSupportedStages (VUID-RuntimeSpirv-None-06343). make_profile and
profile_pinned are cut out of the source and compiled with a stand-in device; each run sets the device's properties and
checks the profile against them.

	python -I -B tests/test_shader_profile_caps.py --cxx clang++
"""

import argparse
import os
from pathlib import Path
import subprocess
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
VK = ROOT / "gpu/vk"

FIXTURE = r'''
#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdlib>
#include "shader_recompiler/profile.h"
#include "shader_recompiler/stage.h"

enum VkShaderStageFlagBits : uint32_t {
	VK_SHADER_STAGE_VERTEX_BIT = 1,
	VK_SHADER_STAGE_TESSELLATION_CONTROL_BIT = 2,
	VK_SHADER_STAGE_TESSELLATION_EVALUATION_BIT = 4,
	VK_SHADER_STAGE_GEOMETRY_BIT = 8,
	VK_SHADER_STAGE_FRAGMENT_BIT = 16,
	VK_SHADER_STAGE_COMPUTE_BIT = 32,
	VK_SHADER_STAGE_TASK_BIT_EXT = 64,
	VK_SHADER_STAGE_MESH_BIT_EXT = 128,
	VK_SHADER_STAGE_ALL = 0x7fffffff,
};
using VkShaderStageFlags = uint32_t;
using VkBool32 = uint32_t;
constexpr VkBool32 VK_FALSE = 0;
enum VkShaderFloatControlsIndependence {
	VK_SHADER_FLOAT_CONTROLS_INDEPENDENCE_32_BIT_ONLY = 0,
	VK_SHADER_FLOAT_CONTROLS_INDEPENDENCE_ALL = 1,
	VK_SHADER_FLOAT_CONTROLS_INDEPENDENCE_NONE = 2,
};
constexpr uint32_t VK_DRIVER_ID_AMD_PROPRIETARY = 1;
struct {
	uint32_t sg32_stages = 0, subgroup_size = 32, subgroup_max = 32, driver_id = 0, max_workgroup_subgroups = 32;
	VkShaderStageFlags subgroup_stages = 0;
	struct {
		VkShaderFloatControlsIndependence denormBehaviorIndependence{}, roundingModeIndependence{};
		VkBool32 shaderSignedZeroInfNanPreserveFloat16{}, shaderSignedZeroInfNanPreserveFloat32{},
			shaderSignedZeroInfNanPreserveFloat64{}, shaderDenormPreserveFloat16{}, shaderDenormPreserveFloat32{},
			shaderDenormPreserveFloat64{}, shaderDenormFlushToZeroFloat16{}, shaderDenormFlushToZeroFloat32{},
			shaderDenormFlushToZeroFloat64{};
	} float_controls;
	struct {
		bool shaderFloat16 = true;
		bool shaderStorageImageArrayNonUniformIndexing = true, shaderUniformTexelBufferArrayNonUniformIndexing = true,
			 shaderStorageTexelBufferArrayNonUniformIndexing = true;
		VkBool32 shaderBufferInt64Atomics{};
	} vk12;
	bool float_atomic_add = false;
	struct { struct { uint64_t minStorageBufferOffsetAlignment = 16; } limits; } props;
	struct { bool gather_offset = false; } fixes;
} dev;
'''

CASES = r'''
int main(int argc, char** argv) {
	assert(argc == 3);
	using Shader::Stage;
	const bool on = argv[1][0] == '1';
	dev.subgroup_stages = uint32_t(std::strtoul(argv[2], nullptr, 0));
	auto& fc = dev.float_controls;
	fc.denormBehaviorIndependence = on ? VK_SHADER_FLOAT_CONTROLS_INDEPENDENCE_ALL : VK_SHADER_FLOAT_CONTROLS_INDEPENDENCE_NONE;
	fc.roundingModeIndependence = fc.denormBehaviorIndependence;
	fc.shaderSignedZeroInfNanPreserveFloat16 = fc.shaderSignedZeroInfNanPreserveFloat32 = on;
	fc.shaderDenormPreserveFloat32 = fc.shaderDenormFlushToZeroFloat32 = on;
	dev.vk12.shaderBufferInt64Atomics = on;
	const Shader::Profile& p = profile();
	assert(p.support_fp32_signed_zero_nan_preserve == on);
	assert(p.support_fp16_signed_zero_nan_preserve == on);
	assert(p.support_fp32_denorm_preserve == on);
	assert(p.support_fp32_denorm_flush == on);
	assert(p.support_separate_denorm_behavior == on);
	assert(p.support_int64_atomics == on);
	const auto has = [&](VkShaderStageFlags bit) { return (dev.subgroup_stages & bit) != 0; };
	assert(p.SupportsSubgroupStage(Stage::VertexA) == has(VK_SHADER_STAGE_VERTEX_BIT));
	assert(p.SupportsSubgroupStage(Stage::VertexB) == has(VK_SHADER_STAGE_VERTEX_BIT));
	assert(p.SupportsSubgroupStage(Stage::TessellationControl) == has(VK_SHADER_STAGE_TESSELLATION_CONTROL_BIT));
	assert(p.SupportsSubgroupStage(Stage::TessellationEval) == has(VK_SHADER_STAGE_TESSELLATION_EVALUATION_BIT));
	assert(p.SupportsSubgroupStage(Stage::Geometry) == has(VK_SHADER_STAGE_GEOMETRY_BIT));
	assert(p.SupportsSubgroupStage(Stage::Fragment) == has(VK_SHADER_STAGE_FRAGMENT_BIT));
	assert(p.SupportsSubgroupStage(Stage::Compute) == has(VK_SHADER_STAGE_COMPUTE_BIT));
	assert(p.SupportsSubgroupStage(Stage::Task) == has(VK_SHADER_STAGE_TASK_BIT_EXT));
	assert(p.SupportsSubgroupStage(Stage::Mesh) == has(VK_SHADER_STAGE_MESH_BIT_EXT));
	// (the compute profile: the mesh fallback runs task and mesh programs as compute shaders, so the compute stage's support)
	const Shader::Profile& c = profile_pinned();
	for (const Stage s : {Stage::Compute, Stage::Task, Stage::Mesh})
		assert(c.SupportsSubgroupStage(s) == has(VK_SHADER_STAGE_COMPUTE_BIT));
}
'''


def section(source, start, end):
	first = source.index(start)
	return source[first:source.index(end, first)]


def main():
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument("--cxx", default="clang++")
	args = parser.parse_args()
	shader = (VK / "vk_shader.cpp").read_text(encoding="utf-8")
	internal = (VK / "vk_internal.h").read_text(encoding="utf-8")
	fixture = (FIXTURE + section(internal, "inline bool pins_32(" if "inline bool pins_32(" in internal else "inline uint64_t storage_binding_align()", "/// A range of guest memory")
			   + section(shader, "Shader::Profile make_profile(", "Shader::HostTranslateInfo host_info()")
			   + section(shader, "const Shader::Profile& profile()", "/// The profile of a graphics stage") + CASES)
	with tempfile.TemporaryDirectory(prefix="longshot-profile-caps-") as tmp:
		cpp, exe = Path(tmp) / "fixture.cpp", Path(tmp) / "fixture.exe"
		cpp.write_text(fixture, encoding="utf-8")
		subprocess.run([args.cxx, "-std=c++20", "-I", str(VK / "eden"), "-I", str(ROOT / "gpu/third_party/suyu"),
						str(cpp), "-o", str(exe)], check=True, timeout=60)
		env = {k: v for k, v in os.environ.items() if not k.startswith("RX_VK_")}
		# (a fresh process per device: the profiles are made once. Inputs: every capability on or off, supportedStages)
		cases = [("1", "0xff"), ("0", "0xff"), ("1", "0x30"), ("0", "0x20"), ("1", "0x10"), ("0", "0")]
		for on, stages in cases:
			result = subprocess.run([str(exe), on, stages], env=env, timeout=10)
			if result.returncode != 0:
				raise SystemExit(f"FAIL: the profile does not follow the device (capabilities {on}, subgroup stages {stages})")
	print("float controls, 64-bit atomics and subgroup stages follow the device ok")


if __name__ == "__main__":
	main()
