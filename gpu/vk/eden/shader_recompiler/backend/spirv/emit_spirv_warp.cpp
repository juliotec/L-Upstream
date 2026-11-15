// SPDX-FileCopyrightText: Copyright 2026 Eden Emulator Project
// SPDX-License-Identifier: GPL-3.0-or-later

// SPDX-FileCopyrightText: Copyright 2021 yuzu Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include "shader_recompiler/backend/spirv/emit_spirv_instructions.h"
#include "shader_recompiler/backend/spirv/spirv_emit_context.h"
#include "shader_recompiler/backend/spirv/mma_layout.h"
#include "shader_recompiler/backend/spirv/mma_software.h"

namespace Shader::Backend::SPIRV {
namespace {
Id SubgroupScope(EmitContext& ctx) {
    return ctx.Const(static_cast<u32>(spv::Scope::Subgroup));
}

bool StageSupportsSubgroups(EmitContext& ctx) {
    return ctx.profile.SupportsSubgroupStage(ctx.stage);
}

Id GetThreadId(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.u32_zero_value;
    }
    return ctx.OpLoad(ctx.U32[1], ctx.subgroup_local_invocation_id);
}

Id WarpExtract(EmitContext& ctx, Id value) {
    const Id thread_id{GetThreadId(ctx)};
    const Id local_index{ctx.OpShiftRightArithmetic(ctx.U32[1], thread_id, ctx.Const(5U))};
    if (ctx.profile.has_broken_spirv_subgroup_mask_vector_extract_dynamic) {
        const Id c0_sel{ctx.OpSelect(ctx.U32[1], ctx.OpIEqual(ctx.U1, local_index, ctx.Const(0U)),
                                     ctx.OpCompositeExtract(ctx.U32[1], value, 0U), ctx.Const(0U))};
        const Id c1_sel{ctx.OpSelect(ctx.U32[1], ctx.OpIEqual(ctx.U1, local_index, ctx.Const(1U)),
                                     ctx.OpCompositeExtract(ctx.U32[1], value, 1U), ctx.Const(0U))};
        const Id c2_sel{ctx.OpSelect(ctx.U32[1], ctx.OpIEqual(ctx.U1, local_index, ctx.Const(2U)),
                                     ctx.OpCompositeExtract(ctx.U32[1], value, 2U), ctx.Const(0U))};
        const Id c3_sel{ctx.OpSelect(ctx.U32[1], ctx.OpIEqual(ctx.U1, local_index, ctx.Const(3U)),
                                     ctx.OpCompositeExtract(ctx.U32[1], value, 3U), ctx.Const(0U))};
        const Id c0_or_c1{ctx.OpBitwiseOr(ctx.U32[1], c0_sel, c1_sel)};
        const Id c2_or_c3{ctx.OpBitwiseOr(ctx.U32[1], c2_sel, c3_sel)};
        const Id c0_or_c1_or_c2_or_c3{ctx.OpBitwiseOr(ctx.U32[1], c0_or_c1, c2_or_c3)};
        return c0_or_c1_or_c2_or_c3;
    } else {
        return ctx.OpVectorExtractDynamic(ctx.U32[1], value, local_index);
    }
}

Id LoadMask(EmitContext& ctx, Id mask) {
    const Id value{ctx.OpLoad(ctx.U32[4], mask)};
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return ctx.OpCompositeExtract(ctx.U32[1], value, 0U);
    }
    return WarpExtract(ctx, value);
}

void SetInBoundsFlag(IR::Inst* inst, Id result) {
    IR::Inst* const in_bounds{inst->GetAssociatedPseudoOperation(IR::Opcode::GetInBoundsFromOp)};
    if (!in_bounds) {
        return;
    }
    in_bounds->SetDefinition(result);
    in_bounds->Invalidate();
}

Id ComputeMinThreadId(EmitContext& ctx, Id thread_id, Id segmentation_mask) {
    return ctx.OpBitwiseAnd(ctx.U32[1], thread_id, segmentation_mask);
}

Id ComputeMaxThreadId(EmitContext& ctx, Id min_thread_id, Id clamp, Id not_seg_mask) {
    return ctx.OpBitwiseOr(ctx.U32[1], min_thread_id,
                           ctx.OpBitwiseAnd(ctx.U32[1], clamp, not_seg_mask));
}

Id GetMaxThreadId(EmitContext& ctx, Id thread_id, Id clamp, Id segmentation_mask) {
    const Id not_seg_mask{ctx.OpNot(ctx.U32[1], segmentation_mask)};
    const Id min_thread_id{ComputeMinThreadId(ctx, thread_id, segmentation_mask)};
    return ComputeMaxThreadId(ctx, min_thread_id, clamp, not_seg_mask);
}

Id HostThreadId(EmitContext& ctx, Id thread_id) {
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return thread_id;
    }
    const Id partition_idx{ctx.OpShiftRightLogical(ctx.U32[1], GetThreadId(ctx), ctx.Const(5u))};
    const Id partition_base{ctx.OpShiftLeftLogical(ctx.U32[1], partition_idx, ctx.Const(5u))};
    return ctx.OpIAdd(ctx.U32[1], thread_id, partition_base);
}

Id GuestLane(EmitContext& ctx, Id index) {
    return ctx.OpBitwiseAnd(ctx.U32[1], index, ctx.Const(31U));
}

bool UseShuffleFallback(EmitContext& ctx) {
    return ctx.profile.has_broken_spirv_subgroup_shuffle ||
           (ctx.profile.has_broken_spirv_fragment_shuffle && ctx.stage == Stage::Fragment);
}

Id ShuffleAbsolute(EmitContext& ctx, Id value, Id src_thread_id, bool broadcast) {
    if (!broadcast) {
        return ctx.OpGroupNonUniformShuffle(ctx.U32[1], SubgroupScope(ctx), value, src_thread_id);
    }
    Id result{ctx.u32_zero_value};
    for (u32 lane = 0; lane < ctx.profile.max_subgroup_size; ++lane) {
        const Id read{
            ctx.OpGroupNonUniformBroadcast(ctx.U32[1], SubgroupScope(ctx), value, ctx.Const(lane))};
        const Id matches{ctx.OpIEqual(ctx.U1, src_thread_id, ctx.Const(lane))};
        result = ctx.OpSelect(ctx.U32[1], matches, read, result);
    }
    return result;
}

Id ShuffleRelative(EmitContext& ctx, Id value, Id delta, Id src_thread_id, spv::Op op) {
    const bool broadcast{UseShuffleFallback(ctx)};
    if (!ctx.profile.support_shuffle_relative || broadcast) {
        return ShuffleAbsolute(ctx, value, HostThreadId(ctx, src_thread_id), broadcast);
    }
    const Id result{ctx.OpGroupNonUniformShuffleXor(ctx.U32[1], SubgroupScope(ctx), value, delta)};
    ctx.opcode_rewrites.emplace_back(result.value, op);
    return result;
}

Id BroadcastLane(EmitContext& ctx, Id value, u32 lane) {
    Id result{
        ctx.OpGroupNonUniformBroadcast(ctx.U32[1], SubgroupScope(ctx), value, ctx.Const(lane))};
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return result;
    }
    const Id partition_idx{ctx.OpShiftRightLogical(ctx.U32[1], GetThreadId(ctx), ctx.Const(5u))};
    for (u32 base = 32; base < ctx.profile.max_subgroup_size; base += 32) {
        const Id read{ctx.OpGroupNonUniformBroadcast(ctx.U32[1], SubgroupScope(ctx), value,
                                                     ctx.Const(base + lane))};
        const Id matches{ctx.OpIEqual(ctx.U1, partition_idx, ctx.Const(base >> 5))};
        result = ctx.OpSelect(ctx.U32[1], matches, read, result);
    }
    return result;
}
} // Anonymous namespace

Id EmitLaneId(EmitContext& ctx) {
    const Id id{GetThreadId(ctx)};
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return id;
    }
    return ctx.OpBitwiseAnd(ctx.U32[1], id, ctx.Const(31U));
}

Id EmitVoteAll(EmitContext& ctx, Id pred) {
    if (!StageSupportsSubgroups(ctx)) {
        return pred;
    }
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return ctx.OpGroupNonUniformAll(ctx.U1, SubgroupScope(ctx), pred);
    }
    const Id mask_ballot{
        ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), ctx.true_value)};
    const Id active_mask{WarpExtract(ctx, mask_ballot)};
    const Id ballot{
        WarpExtract(ctx, ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), pred))};
    const Id lhs{ctx.OpBitwiseAnd(ctx.U32[1], ballot, active_mask)};
    return ctx.OpIEqual(ctx.U1, lhs, active_mask);
}

Id EmitVoteAny(EmitContext& ctx, Id pred) {
    if (!StageSupportsSubgroups(ctx)) {
        return pred;
    }
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return ctx.OpGroupNonUniformAny(ctx.U1, SubgroupScope(ctx), pred);
    }
    const Id mask_ballot{
        ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), ctx.true_value)};
    const Id active_mask{WarpExtract(ctx, mask_ballot)};
    const Id ballot{
        WarpExtract(ctx, ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), pred))};
    const Id lhs{ctx.OpBitwiseAnd(ctx.U32[1], ballot, active_mask)};
    return ctx.OpINotEqual(ctx.U1, lhs, ctx.u32_zero_value);
}

Id EmitVoteEqual(EmitContext& ctx, Id pred) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.true_value;
    }
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return ctx.OpGroupNonUniformAllEqual(ctx.U1, SubgroupScope(ctx), pred);
    }
    const Id mask_ballot{
        ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), ctx.true_value)};
    const Id active_mask{WarpExtract(ctx, mask_ballot)};
    const Id ballot{
        WarpExtract(ctx, ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), pred))};
    const Id lhs{ctx.OpBitwiseXor(ctx.U32[1], ballot, active_mask)};
    return ctx.OpLogicalOr(ctx.U1, ctx.OpIEqual(ctx.U1, lhs, ctx.u32_zero_value),
                           ctx.OpIEqual(ctx.U1, lhs, active_mask));
}

Id EmitSubgroupBallot(EmitContext& ctx, Id pred) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.OpSelect(ctx.U32[1], pred, ctx.Const(1u), ctx.u32_zero_value);
    }
    const Id ballot{ctx.OpGroupNonUniformBallot(ctx.U32[4], SubgroupScope(ctx), pred)};
    if (!ctx.profile.warp_size_potentially_larger_than_guest) {
        return ctx.OpCompositeExtract(ctx.U32[1], ballot, 0U);
    }
    return WarpExtract(ctx, ballot);
}

Id EmitSubgroupEqMask(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.Const(1u);
    }
    return LoadMask(ctx, ctx.subgroup_mask_eq);
}

Id EmitSubgroupLtMask(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.u32_zero_value;
    }
    return LoadMask(ctx, ctx.subgroup_mask_lt);
}

Id EmitSubgroupLeMask(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.Const(1u);
    }
    return LoadMask(ctx, ctx.subgroup_mask_le);
}

Id EmitSubgroupGtMask(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.u32_zero_value;
    }
    return LoadMask(ctx, ctx.subgroup_mask_gt);
}

Id EmitSubgroupGeMask(EmitContext& ctx) {
    if (!StageSupportsSubgroups(ctx)) {
        return ctx.Const(1u);
    }
    return LoadMask(ctx, ctx.subgroup_mask_ge);
}

Id EmitShuffleIndex(EmitContext& ctx, IR::Inst* inst, Id value, Id index, Id clamp,
                    Id segmentation_mask) {
    const Id not_seg_mask{ctx.OpNot(ctx.U32[1], segmentation_mask)};
    const Id thread_id{EmitLaneId(ctx)};
    const Id min_thread_id{ComputeMinThreadId(ctx, thread_id, segmentation_mask)};
    const Id max_thread_id{ComputeMaxThreadId(ctx, min_thread_id, clamp, not_seg_mask)};

    const Id lhs{ctx.OpBitwiseAnd(ctx.U32[1], GuestLane(ctx, index), not_seg_mask)};
    const Id src_thread_id{ctx.OpBitwiseOr(ctx.U32[1], lhs, min_thread_id)};
    const Id in_range{ctx.OpSLessThanEqual(ctx.U1, src_thread_id, max_thread_id)};

    SetInBoundsFlag(inst, in_range);
    if (!StageSupportsSubgroups(ctx)) {
        return value;
    }
    const IR::Value lane{inst->Arg(1).Resolve()};
    const IR::Value segment{inst->Arg(3).Resolve()};
    if (lane.IsImmediate() && segment.IsImmediate() && segment.U32() == 0) {
        return ctx.OpSelect(ctx.U32[1], in_range, BroadcastLane(ctx, value, lane.U32() & 31),
                            value);
    }
    const Id shuffled{
        ShuffleAbsolute(ctx, value, HostThreadId(ctx, src_thread_id), UseShuffleFallback(ctx))};
    return ctx.OpSelect(ctx.U32[1], in_range, shuffled, value);
}

Id EmitShuffleUp(EmitContext& ctx, IR::Inst* inst, Id value, Id index, Id clamp,
                 Id segmentation_mask) {
    if (!StageSupportsSubgroups(ctx)) {
        SetInBoundsFlag(inst, ctx.false_value);
        return value;
    }
    const Id delta{GuestLane(ctx, index)};
    const Id thread_id{EmitLaneId(ctx)};
    const Id max_thread_id{GetMaxThreadId(ctx, thread_id, clamp, segmentation_mask)};
    const Id src_thread_id{ctx.OpISub(ctx.U32[1], thread_id, delta)};
    const Id in_range{ctx.OpSGreaterThanEqual(ctx.U1, src_thread_id, max_thread_id)};

    SetInBoundsFlag(inst, in_range);
    const Id shuffled{ShuffleRelative(ctx, value, delta, src_thread_id,
                                      spv::Op::OpGroupNonUniformShuffleUp)};
    return ctx.OpSelect(ctx.U32[1], in_range, shuffled, value);
}

Id EmitShuffleDown(EmitContext& ctx, IR::Inst* inst, Id value, Id index, Id clamp,
                   Id segmentation_mask) {
    if (!StageSupportsSubgroups(ctx)) {
        SetInBoundsFlag(inst, ctx.false_value);
        return value;
    }
    const Id delta{GuestLane(ctx, index)};
    const Id thread_id{EmitLaneId(ctx)};
    const Id max_thread_id{GetMaxThreadId(ctx, thread_id, clamp, segmentation_mask)};
    const Id src_thread_id{ctx.OpIAdd(ctx.U32[1], thread_id, delta)};
    const Id in_range{ctx.OpSLessThanEqual(ctx.U1, src_thread_id, max_thread_id)};

    SetInBoundsFlag(inst, in_range);
    const Id shuffled{ShuffleRelative(ctx, value, delta, src_thread_id,
                                      spv::Op::OpGroupNonUniformShuffleDown)};
    return ctx.OpSelect(ctx.U32[1], in_range, shuffled, value);
}

Id EmitShuffleButterfly(EmitContext& ctx, IR::Inst* inst, Id value, Id index, Id clamp,
                        Id segmentation_mask) {
    if (!StageSupportsSubgroups(ctx)) {
        SetInBoundsFlag(inst, ctx.false_value);
        return value;
    }
    const Id mask{GuestLane(ctx, index)};
    const Id thread_id{EmitLaneId(ctx)};
    const Id max_thread_id{GetMaxThreadId(ctx, thread_id, clamp, segmentation_mask)};
    const Id src_thread_id{ctx.OpBitwiseXor(ctx.U32[1], thread_id, mask)};
    const Id in_range{ctx.OpSLessThanEqual(ctx.U1, src_thread_id, max_thread_id)};

    SetInBoundsFlag(inst, in_range);
    // (longshot) through the broadcasts where shuffles are broken (UseShuffleFallback: AMD's fragment stages, Lichii #273),
    // as the absolute and relative shuffles are; #273 left SHFL.BFLY native, and some of Kokiri Forest's plants use it
    const Id shuffled{UseShuffleFallback(ctx)
                          ? ShuffleAbsolute(ctx, value, HostThreadId(ctx, src_thread_id), true)
                          : ctx.OpGroupNonUniformShuffleXor(ctx.U32[1], SubgroupScope(ctx), value, mask)};
    return ctx.OpSelect(ctx.U32[1], in_range, shuffled, value);
}

Id EmitQuadBroadcast(EmitContext& ctx, Id value, Id lane) {
    if (ctx.profile.support_quad_shuffles) {
        return ctx.OpGroupNonUniformQuadBroadcast(ctx.U32[1], SubgroupScope(ctx), value, lane);
    }
    const Id base{ctx.OpBitwiseAnd(ctx.U32[1], GetThreadId(ctx), ctx.Const(~3u))};
    const Id local_lane{ctx.OpBitwiseAnd(ctx.U32[1], lane, ctx.Const(3u))};
    const Id src_thread_id{ctx.OpBitwiseOr(ctx.U32[1], base, local_lane)};
    return ShuffleAbsolute(ctx, value, src_thread_id,
                           ctx.profile.has_broken_spirv_subgroup_shuffle);
}

Id EmitQuadSwap(EmitContext& ctx, Id value, Id direction) {
    if (ctx.profile.support_quad_shuffles) {
        const Id result{
            ctx.OpGroupNonUniformQuadBroadcast(ctx.U32[1], SubgroupScope(ctx), value, direction)};
        ctx.opcode_rewrites.emplace_back(result.value, spv::Op::OpGroupNonUniformQuadSwap);
        return result;
    }
    const Id xor_mask{ctx.OpIAdd(ctx.U32[1], direction, ctx.Const(1u))};
    // (longshot) not through UseShuffleFallback's broadcasts: a program with quad shuffles alone (uses_quad_shuffles) has
    // neither the lane id input nor the ballot capability they need, and the SPIR-V made with them crashed the start
    return ctx.OpGroupNonUniformShuffleXor(ctx.U32[1], SubgroupScope(ctx), value, xor_mask);
}

Id EmitFSwizzleAdd(EmitContext& ctx, Id op_a, Id op_b, Id swizzle) {
    const Id three{ctx.Const(3U)};
    Id mask{GetThreadId(ctx)};
    mask = ctx.OpBitwiseAnd(ctx.U32[1], mask, three);
    mask = ctx.OpShiftLeftLogical(ctx.U32[1], mask, ctx.Const(1U));
    mask = ctx.OpShiftRightLogical(ctx.U32[1], swizzle, mask);
    mask = ctx.OpBitwiseAnd(ctx.U32[1], mask, three);

    const Id modifier_a{ctx.OpVectorExtractDynamic(ctx.F32[1], ctx.fswzadd_lut_a, mask)};
    const Id modifier_b{ctx.OpVectorExtractDynamic(ctx.F32[1], ctx.fswzadd_lut_b, mask)};

    const Id result_a{ctx.OpFMul(ctx.F32[1], op_a, modifier_a)};
    const Id result_b{ctx.OpFMul(ctx.F32[1], op_b, modifier_b)};
    return ctx.OpFAdd(ctx.F32[1], result_a, result_b);
}

Id EmitDPdxFine(EmitContext& ctx, Id op_a) {
    return ctx.OpDPdxFine(ctx.F32[1], op_a);
}

Id EmitDPdyFine(EmitContext& ctx, Id op_a) {
    return ctx.OpDPdyFine(ctx.F32[1], op_a);
}

Id EmitDPdxCoarse(EmitContext& ctx, Id op_a) {
    return ctx.OpDPdxCoarse(ctx.F32[1], op_a);
}

Id EmitDPdyCoarse(EmitContext& ctx, Id op_a) {
    return ctx.OpDPdyCoarse(ctx.F32[1], op_a);
}

// (longshot) HMMA in software (mma_software.h): the 32 lanes' fragments through subgroup operations in one helper function,
// accumulated with FMA in f32 and rounded to f16 after every 8 columns of K, as HMMA.F16 does. It needs no cooperative matrices
// (RX_VK_MMA_SOFTWARE=1, vk_coopmat.cpp); the form is the one vk_coopmat.cpp's self-test chose for this device (mma_software_form).
void DefineSoftwareMatrixFunction(EmitContext& ctx) {
    ctx.AddCapability(spv::Capability::GroupNonUniform);
    ctx.AddCapability(spv::Capability::GroupNonUniformShuffle);
    if (!ctx.profile.SupportsSubgroupStage(ctx.stage) || ctx.profile.has_broken_spirv_subgroup_shuffle)
        throw NotImplementedException("Software HMMA requires subgroup shuffle support");
    const auto form = static_cast<SoftwareMmaForm>(mma_software_form);
    const bool quad = form != SoftwareMmaForm::Loop && mma_software_quad;
    if (quad) ctx.AddCapability(spv::Capability::GroupNonUniformQuad);
    const SoftwareMmaTypes types{ctx.U32[1], ctx.U32[2], ctx.U32[4], ctx.F32[1], ctx.F32[2], ctx.F16[1], ctx.U1};
    ctx.software_matrix_function = DefineSoftwareMma(ctx, types, ctx.subgroup_local_invocation_id, form, quad);
}


// (longshot, AMD RDNA 3/4) HMMA_nx2 on 16x16x16 cooperative matrices (mma_layout.h mma_wmma16). Each subgroup has 512
// words of workgroup memory past the program's own: A (row major), B's K halves 0-7 and 8-15 (column major, the
// other half's rows zero) and C/D (row major), every 16x16 f16. The lanes write their fragments at the console's
// positions (PTX m16n8k16) and zeros where B and C have no columns 8-15, the device loads the tiles in its own
// layout, D = A*B_hi + (A*B_lo + C) is rounded to f16 between the two as HMMA.F16 does, and each lane reads its two
// words of D back. False when the tiles do not fit the device's shared memory: the program then takes the software
// multiply.
bool DefineWmma16MatrixFunction(EmitContext& ctx, const IR::Program& program) {
    const u32 threads = std::max(1U, program.workgroup_size[0] * program.workgroup_size[1] * program.workgroup_size[2]);
    const u32 subgroups = (threads + 31) / 32;
    const bool explicit_layout = ctx.uses_explicit_workgroup_layout && program.shared_memory_size != 0;
    const u32 offset_bytes = explicit_layout ? (program.shared_memory_size + 15U) & ~15U : 0U;
    const u32 own_bytes = explicit_layout ? offset_bytes : (program.shared_memory_size + 3U) & ~3U;
    const u32 words = subgroups * mma_wmma16_tile_words;
    if (mma_wmma16_shared_limit != 0 && own_bytes + words * 4 + 4 > mma_wmma16_shared_limit) return false;

    const Id u32ty{ctx.U32[1]};
    const Id word_pointer{ctx.TypePointer(spv::StorageClass::Workgroup, u32ty)};
    const Id array_type{ctx.TypeArray(u32ty, ctx.Const(words))};
    Id scratch{};
    if (explicit_layout) {
        // (all the Workgroup variables are blocks over the same memory: these words follow the program's. The second
        // member keeps the structure distinct from the program's own single-member ones, decorated at offset 0)
        ctx.Decorate(array_type, spv::Decoration::ArrayStride, 4U);
        const Id block{ctx.TypeStruct(array_type, u32ty)};
        ctx.MemberDecorate(block, 0U, spv::Decoration::Offset, offset_bytes);
        ctx.MemberDecorate(block, 1U, spv::Decoration::Offset, offset_bytes + words * 4);
        ctx.Decorate(block, spv::Decoration::Block);
        scratch = ctx.AddGlobalVariable(ctx.TypePointer(spv::StorageClass::Workgroup, block),
                                        spv::StorageClass::Workgroup);
        ctx.Decorate(scratch, spv::Decoration::Aliased);
    } else {
        scratch = ctx.AddGlobalVariable(ctx.TypePointer(spv::StorageClass::Workgroup, array_type),
                                        spv::StorageClass::Workgroup);
    }
    ctx.interfaces.push_back(scratch);
    ctx.AddCapability(spv::Capability::GroupNonUniform);
    const Id subgroup_id{ctx.AddGlobalVariable(ctx.TypePointer(spv::StorageClass::Input, u32ty),
                                               spv::StorageClass::Input)};
    ctx.Decorate(subgroup_id, spv::Decoration::BuiltIn, spv::BuiltIn::SubgroupId);
    ctx.interfaces.push_back(subgroup_id);

    const auto imm = [&](u32 v) { return ctx.Const(v); };
    const auto add = [&](Id a, Id b) { return ctx.OpIAdd(u32ty, a, b); };
    const auto at = [&](Id index) {
        return explicit_layout ? ctx.OpAccessChain(word_pointer, scratch, ctx.u32_zero_value, index)
                               : ctx.OpAccessChain(word_pointer, scratch, index);
    };
    const auto store = [&](Id index, Id value) { ctx.OpStore(at(index), value, spv::MemoryAccessMask::NonPrivatePointer); };
    const auto load = [&](Id index) { return ctx.OpLoad(u32ty, at(index), spv::MemoryAccessMask::NonPrivatePointer); };
    const Id subgroup_scope{imm(static_cast<u32>(spv::Scope::Subgroup))};
    const auto barrier = [&] {
        ctx.OpControlBarrier(subgroup_scope, subgroup_scope,
                             imm(static_cast<u32>(spv::MemorySemanticsMask::AcquireRelease |
                                                  spv::MemorySemanticsMask::WorkgroupMemory |
                                                  spv::MemorySemanticsMask::MakeAvailable |
                                                  spv::MemorySemanticsMask::MakeVisible)));
    };
    const Id f16{ctx.F16[1]};
    const auto type = [&](spv::CooperativeMatrixUse use) {
        return ctx.TypeCooperativeMatrixKHR(f16, subgroup_scope, imm(16), imm(16), imm(static_cast<u32>(use)));
    };
    const Id a_type{type(spv::CooperativeMatrixUse::MatrixAKHR)};
    const Id b_type{type(spv::CooperativeMatrixUse::MatrixBKHR)};
    const Id c_type{type(spv::CooperativeMatrixUse::MatrixAccumulatorKHR)};

    const Id fn_type{ctx.TypeFunction(ctx.U32[2], ctx.U32[4], ctx.U32[2], ctx.U32[2])};
    const Id fn{ctx.OpFunction(ctx.U32[2], spv::FunctionControlMask::DontInline, fn_type)};
    ctx.wmma16_function = fn;
    const Id matrix_a{ctx.OpFunctionParameter(ctx.U32[4])};
    const Id matrix_b{ctx.OpFunctionParameter(ctx.U32[2])};
    const Id matrix_c{ctx.OpFunctionParameter(ctx.U32[2])};
    ctx.AddLabel();
    const Id lane{ctx.OpBitwiseAnd(u32ty, ctx.OpLoad(u32ty, ctx.subgroup_local_invocation_id), imm(31))};
    const Id base{ctx.OpIMul(u32ty, ctx.OpLoad(u32ty, subgroup_id), imm(mma_wmma16_tile_words))};
    const Id g{ctx.OpShiftRightLogical(u32ty, lane, imm(2))};
    const Id i{ctx.OpBitwiseAnd(u32ty, lane, imm(3))};
    // the word of row (or column) g (+8) at word i (+4) of a region
    const auto word = [&](u32 region, u32 plus_rows, u32 plus_words) {
        return add(add(base, imm(region * 128 + plus_rows * 8 + plus_words)),
                   add(ctx.OpShiftLeftLogical(u32ty, g, imm(3)), i));
    };
    const Id zero{ctx.u32_zero_value};
    const auto part = [&](Id words, u32 index) { return ctx.OpCompositeExtract(u32ty, words, index); };
    constexpr u32 region_a = 0, region_b_lo = 1, region_b_hi = 2, region_c = 3;
    // trailing reads of the last multiply in this subgroup are done before these writes (the barrier at the end)
    store(word(region_a, 0, 0), part(matrix_a, 0));
    store(word(region_a, 8, 0), part(matrix_a, 1));
    store(word(region_a, 0, 4), part(matrix_a, 2));
    store(word(region_a, 8, 4), part(matrix_a, 3));
    store(word(region_b_lo, 0, 0), part(matrix_b, 0));
    store(word(region_b_lo, 0, 4), mma_wmma16_fast ? part(matrix_b, 1) : zero);
    store(word(region_b_lo, 8, 0), zero);
    store(word(region_b_lo, 8, 4), zero);
    if (!mma_wmma16_fast) {
        store(word(region_b_hi, 0, 0), zero);
        store(word(region_b_hi, 0, 4), part(matrix_b, 1));
        store(word(region_b_hi, 8, 0), zero);
        store(word(region_b_hi, 8, 4), zero);
    }
    store(word(region_c, 0, 0), part(matrix_c, 0));
    store(word(region_c, 8, 0), part(matrix_c, 1));
    store(word(region_c, 0, 4), zero);
    store(word(region_c, 8, 4), zero);
    barrier();
    const Id row_major{imm(0)}, column_major{imm(1)}, stride{imm(8)};
    const auto tile = [&](Id matrix_type, u32 region, Id layout) {
        return ctx.OpCooperativeMatrixLoadKHR(matrix_type, at(add(base, imm(region * 128))), layout, stride,
                                              spv::MemoryAccessMask::NonPrivatePointer);
    };
    const Id a{tile(a_type, region_a, row_major)};
    Id d{ctx.OpCooperativeMatrixMulAddKHR(c_type, a, tile(b_type, region_b_lo, column_major),
                                           tile(c_type, region_c, row_major))};
    if (!mma_wmma16_fast)
        d = ctx.OpCooperativeMatrixMulAddKHR(c_type, a, tile(b_type, region_b_hi, column_major), d);
    barrier();
    ctx.OpCooperativeMatrixStoreKHR(at(add(base, imm(region_c * 128))), d, row_major, stride,
                                    spv::MemoryAccessMask::NonPrivatePointer);
    barrier();
    const Id d0{load(word(region_c, 0, 0))};
    const Id d1{load(word(region_c, 8, 0))};
    barrier();
    ctx.OpReturnValue(ctx.OpCompositeConstruct(ctx.U32[2], d0, d1));
    ctx.OpFunctionEnd();
    return true;
}


// (longshot) HMMA_nx2: D(16x8) = A(16x16) * B(16x8) + C on the subgroup's cooperative matrices. Each
// lane's fragment registers hold its elements in the console's order (two f16 a word); they go to and come
// from the device's elements in its own order (mma_layout.h, measured by vk_coopmat.cpp).
Id EmitMatrixMulAddF16(EmitContext& ctx, Id a, Id b, Id c) {
    if (ctx.wmma16_function.value != 0) return ctx.OpFunctionCall(ctx.U32[2], ctx.wmma16_function, a, b, c);
    if (ctx.software_matrix_function.value != 0)
        return ctx.OpFunctionCall(ctx.U32[2], ctx.software_matrix_function, a, b, c);
    const Id f16{ctx.F16[1]};
    const Id scope{ctx.Const(static_cast<u32>(spv::Scope::Subgroup))};
    const auto type = [&](u32 rows, u32 columns, spv::CooperativeMatrixUse use) {
        return ctx.TypeCooperativeMatrixKHR(f16, scope, ctx.Const(rows), ctx.Const(columns),
                                            ctx.Const(static_cast<u32>(use)));
    };
    const Id a_type{type(16, 16, spv::CooperativeMatrixUse::MatrixAKHR)};
    const Id b_type{type(16, 8, spv::CooperativeMatrixUse::MatrixBKHR)};
    const Id c_type{type(16, 8, spv::CooperativeMatrixUse::MatrixAccumulatorKHR)};
    const Id zero{ctx.Constant(f16, 0U)};
    const auto load = [&](Id matrix_type, Id words, u32 count, const auto& order) {
        Id m{ctx.OpCompositeConstruct(matrix_type, zero)};
        for (u32 w = 0; w < count; ++w) {
            const Id word{count == 1 ? words : ctx.OpCompositeExtract(ctx.U32[1], words, w)};
            const Id pair{ctx.OpBitcast(ctx.F16[2], word)};
            for (u32 h = 0; h < 2; ++h) {
                m = ctx.OpCompositeInsert(matrix_type, ctx.OpCompositeExtract(f16, pair, h), m, order[w * 2 + h]);
            }
        }
        return m;
    };
    const Id d{ctx.OpCooperativeMatrixMulAddKHR(c_type, load(a_type, a, 4, mma_order_a), load(b_type, b, 2, mma_order_b),
                                                    load(c_type, c, 2, mma_order_c))};
    Id out[2];
    for (u32 w = 0; w < 2; ++w) {
        const Id pair{ctx.OpCompositeConstruct(ctx.F16[2], ctx.OpCompositeExtract(f16, d, mma_order_c[w * 2]),
                                               ctx.OpCompositeExtract(f16, d, mma_order_c[w * 2 + 1]))};
        out[w] = ctx.OpBitcast(ctx.U32[1], pair);
    }
    return ctx.OpCompositeConstruct(ctx.U32[2], out[0], out[1]);
}

} // namespace Shader::Backend::SPIRV
