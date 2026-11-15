"""HMMA_nx2 through 16x16x16 cooperative matrices (AMD RDNA 3/4: mma_layout.h mma_wmma16).

Checks that the pieces are wired (Sirit's store, the probe, the emitter, the program cache's stamp, the network gate),
and runs the tile layout DefineWmma16MatrixFunction emits: the lanes' stores are read from the source, a 16x16x16
multiply is done on the tiles as the device would (row major A and C, column major B, stride 8 words), and each lane's
two words of D must be the console's fragment (PTX m16n8k16) of A*B+C.
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
VK = ROOT / "gpu/vk"
SPIRV = VK / "eden/shader_recompiler/backend/spirv"


def need(ok, why):
    if not ok:
        raise SystemExit("FAIL: " + why)


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def wiring():
    sirit_h = (VK / "sirit/include/sirit/sirit.h").read_text(encoding="utf-8")
    arith = (VK / "sirit/src/instructions/arithmetic.cpp").read_text(encoding="utf-8")
    need("Id OpCooperativeMatrixStoreKHR(" in sirit_h and "Id Module::OpCooperativeMatrixStoreKHR(" in arith,
         "Sirit has no OpCooperativeMatrixStoreKHR")
    store = block(arith, "Id Module::OpCooperativeMatrixStoreKHR(")
    need("OpId{" not in store, "OpCooperativeMatrixStoreKHR has no result id, yet one is written")
    layout = (SPIRV / "mma_layout.h").read_text(encoding="utf-8")
    need("inline bool mma_wmma16 = false;" in layout and "mma_wmma16_tile_words = 4 * 128;" in layout,
         "mma_layout.h lacks mma_wmma16")
    coop = (VK / "vk_coopmat.cpp").read_text(encoding="utf-8")
    init = block(coop, "void coopmat_init(")
    need("shape16" in init and re.search(r"if \(!shape && shape16\) \{[^\n]*\n\s*wmma16_init\(\);", init),
         "coopmat_init does not take the 16x16x16 path when 16x8x16 is missing")
    need("bool test_multiply_wmma16()" in coop and "test_multiply_wmma16()" in block(coop, "void wmma16_init("),
         "wmma16_init does not check the multiply")
    need("extra_interfaces" in block(coop, "std::vector<uint32_t> build_shader("),
         "the probe's workgroup tiles are not entry point interfaces")
    emit = (SPIRV / "emit_spirv.cpp").read_text(encoding="utf-8")
    need("!DefineWmma16MatrixFunction(ctx, program))" in emit, "EmitSPIRV does not define the WMMA16 function")
    ctx_h = (SPIRV / "spirv_emit_context.h").read_text(encoding="utf-8")
    need("Id wmma16_function{};" in ctx_h, "EmitContext has no wmma16_function")
    warp = (SPIRV / "emit_spirv_warp.cpp").read_text(encoding="utf-8")
    mul = block(warp, "Id EmitMatrixMulAddF16(")
    need("ctx.wmma16_function" in mul and "ctx.software_matrix_function.value != 0" in mul,
         "EmitMatrixMulAddF16 does not call the function that was defined")
    shader = (VK / "vk_shader.cpp").read_text(encoding="utf-8")
    need("(mma_wmma16 ? 2048u : 0u)" in shader and "(mma_wmma16_fast ? 4096u : 0u)" in shader,
         "the program cache's stamp does not tell WMMA16 programs apart")
    need("!Shader::Backend::SPIRV::mma_wmma16)" in shader, "the networks are still left out on AMD with WMMA16")
    return warp


def tiles(warp):
    fn = block(warp, "bool DefineWmma16MatrixFunction(")
    stores = re.findall(r"store\(word\((region_\w+), (\d+), (\d+)\), (part\((matrix_\w), (\d)\)|zero|mma_wmma16_fast \? part\(matrix_b, 1\) : zero)\);", fn)
    need(len(stores) >= 14, "the lanes' stores were not found (%d)" % len(stores))
    regions = {"region_a": 0, "region_b_lo": 1, "region_b_hi": 2, "region_c": 3}
    need("stride{imm(8)}" in fn and "region * 128 + plus_rows * 8 + plus_words" in fn, "the tile addressing changed")

    def a_val(r, k): return ((r * 7 + k * 3 + 1) % 5) - 2
    def b_val(k, n): return ((k * 5 + n * 11 + 2) % 4) - 1
    def c_val(r, n): return ((r * 3 + n * 13) % 9) - 4

    # the console's fragments (vk_coopmat.cpp console_a/b/c), as pairs of halves per word
    def frag(t):
        g, i = t // 4, t % 4
        a = [(a_val(g + ((e >> 1) & 1) * 8, 2 * i + (e & 1) + (e >> 2) * 8)) for e in range(8)]
        b = [(b_val(2 * i + (e & 1) + (e >> 1) * 8, g)) for e in range(4)]
        c = [(c_val(g + (e >> 1) * 8, 2 * i + (e & 1))) for e in range(4)]
        return {"a": [a[0:2], a[2:4], a[4:6], a[6:8]], "b": [b[0:2], b[2:4]], "c": [c[0:2], c[2:4]]}

    mem = [[None, None] for _ in range(512)]
    for t in range(32):
        g, i = t // 4, t % 4
        f = frag(t)
        for region, rows, words, value, mat, idx in stores:
            w = regions[region] * 128 + int(rows) * 8 + int(words) + g * 8 + i
            if value == "zero" or value.startswith("mma_wmma16_fast"):
                mem[w] = [0, 0]
            else:
                mem[w] = list(f[mat[-1]][int(idx)])
    need(all(x[0] is not None for x in mem[0:128]) and all(x[0] is not None for x in mem[384:512]),
         "A or C tiles have words no lane writes")

    def half(base, index):
        return mem[base + index // 2][index & 1]

    def row(base, r, c): return half(base, r * 16 + c)
    def col(base, r, c): return half(base, c * 16 + r)

    d = [[sum(row(0, r, k) * (col(128, k, n) + col(256, k, n)) for k in range(16)) + row(384, r, n)
          for n in range(16)] for r in range(16)]
    bad = 0
    for t in range(32):
        g, i = t // 4, t % 4
        for e in range(4):
            r, n = g + (e >> 1) * 8, 2 * i + (e & 1)
            got = d[r][n]   # (D stored row major over C, each lane reads words (g, i) and (g + 8, i))
            want = c_val(r, n) + sum(a_val(r, k) * b_val(k, n) for k in range(16))
            bad += got != want
    need(bad == 0, "%d of 128 elements of D are not A*B+C at the console's positions" % bad)


def main():
    tiles(wiring())
    print("HMMA_nx2 through 16x16x16 cooperative matrices: wired, and the tiles give A*B+C at the console's positions: ok")


if __name__ == "__main__":
    main()
