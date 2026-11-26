// SPDX-License-Identifier: GPL-3.0-or-later
// Which file runs DLSS 5 (vk_dlss.cpp), shared by the renderer, the menu's DLSS 5 model row (gpu/vk/menu/dlss5.cpp) and the
// tests. No Vulkan, NGX or files here.
//
// NVIDIA's own nvngx_dlssnr.dll is its FP8 model, which runs on RTX 50 cards only: NGX loads it (feature 18) through its
// normal API. The community FP16 conversion of that file (a modified NVIDIA file, which NGX refuses) runs on RTX 20, 30, 40
// and 50: Longshot loads it itself (Windows only) and calls it through longshot_nvngx.dll_nr.dll (dlss_nr_forwarder.c), as
// the Bloodborne PC port does. Neither file comes with Longshot: the player puts NVIDIA's in an ngx folder and the FP16 one
// in that folder's fp16 folder (docs/user/dlss5.md). The FP16 file is used only when it is one of kFp16Known.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>

namespace mkwvk::dlss_nr {

enum class Model { none, nvidia, fp16 };

/// The DLSS 5 model row's choices (opts.dlss5_model). Auto is NVIDIA's file on an RTX 50, as the Bloodborne port's Auto
enum ModelChoice { kModelAuto = 0, kModelNvidia = 1, kModelFp16 = 2 };

/// The DLSS 5 files found: NVIDIA's (<ngx folder>/nvngx_dlssnr.dll) and the known FP16 one (<ngx folder>/fp16/nvngx_dlssnr.dll)
struct Files {
    bool nvidia = false, fp16 = false;
};

/// The DLSS 5 model row shows: an RTX 50 with both files, the only case with a choice
inline bool model_row(bool rtx50, Files f) { return rtx50 && f.nvidia && f.fp16; }

/// Which file runs. An RTX 50 with both: the row's choice (Auto is NVIDIA's). Else the FP16 file when it is there (on RTX
/// 20, 30 and 40 NVIDIA's file does not run), else NVIDIA's (where NGX then says whether it runs)
inline Model pick(bool rtx50, Files f, int choice) {
    if (model_row(rtx50, f)) return choice == kModelFp16 ? Model::fp16 : Model::nvidia;
    if (f.fp16) return Model::fp16;
    return f.nvidia ? Model::nvidia : Model::none;
}

/// The FP16 files Longshot runs: their size, then their SHA-256 (lower-case hex). The Bloodborne PC port's known file (its
/// launcher's Dlss5.Fp16Sha256). Any other file in an fp16 folder is not loaded; one of another size is not even read
struct KnownFp16 {
    uint64_t size;
    const char* sha256;
};
inline constexpr KnownFp16 kFp16Known[] = {
    {276162560ull, "cdfa6c1ec784867182164c7836f324f087b5c9f374d0c9fe46f54999feabf3ed"},
};

/// A file of this size may be one of the FP16 files Longshot runs (its hash says whether it is)
inline bool fp16_size_known(uint64_t size) {
    for (const KnownFp16& known : kFp16Known)
        if (known.size == size) return true;
    return false;
}

/// SHA-256 (FIPS 180-4), fed in parts: the FP16 file is hashed once a run as it is read
class Sha256 {
public:
    void update(const void* data, size_t n) {
        const auto* p = static_cast<const uint8_t*>(data);
        bytes_ += n;
        if (used_) {
            const size_t take = n < 64 - used_ ? n : 64 - used_;
            std::memcpy(block_ + used_, p, take);
            used_ += take, p += take, n -= take;
            if (used_ < 64) return;
            compress(block_);
            used_ = 0;
        }
        for (; n >= 64; p += 64, n -= 64) compress(p);
        std::memcpy(block_, p, n);
        used_ = n;
    }
    /// The digest as 64 lower-case hex digits
    std::string hex() {
        const uint64_t bits = bytes_ * 8;
        const uint8_t one = 0x80, zero = 0;
        update(&one, 1);
        while (used_ != 56) update(&zero, 1);
        uint8_t length[8];
        for (int i = 0; i < 8; i++) length[i] = uint8_t(bits >> (56 - 8 * i));
        update(length, 8);
        static const char digits[] = "0123456789abcdef";
        std::string out;
        for (uint32_t word : h_)
            for (int shift = 28; shift >= 0; shift -= 4) out += digits[(word >> shift) & 15];
        return out;
    }

private:
    static uint32_t rotr(uint32_t x, int n) { return (x >> n) | (x << (32 - n)); }
    void compress(const uint8_t* b) {
        static constexpr uint32_t k[64] = {
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
            0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
            0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
            0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
            0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
            0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
            0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2};
        uint32_t w[64];
        for (int i = 0; i < 16; i++)
            w[i] = uint32_t(b[4 * i]) << 24 | uint32_t(b[4 * i + 1]) << 16 | uint32_t(b[4 * i + 2]) << 8 | b[4 * i + 3];
        for (int i = 16; i < 64; i++) {
            const uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            const uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        uint32_t a = h_[0], bb = h_[1], c = h_[2], d = h_[3], e = h_[4], f = h_[5], g = h_[6], h = h_[7];
        for (int i = 0; i < 64; i++) {
            const uint32_t t1 = h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + k[i] + w[i];
            const uint32_t t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & bb) ^ (a & c) ^ (bb & c));
            h = g, g = f, f = e, e = d + t1, d = c, c = bb, bb = a, a = t1 + t2;
        }
        h_[0] += a, h_[1] += bb, h_[2] += c, h_[3] += d, h_[4] += e, h_[5] += f, h_[6] += g, h_[7] += h;
    }
    uint32_t h_[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    uint8_t block_[64]{};
    size_t used_ = 0;
    uint64_t bytes_ = 0;
};

/// A SHA-256 (hex, either case) is one of the FP16 files Longshot runs
inline bool fp16_known(const std::string& hex) {
    if (hex.size() != 64) return false;
    for (const KnownFp16& known : kFp16Known) {
        bool same = true;
        for (size_t i = 0; i < 64 && same; i++) {
            const char c = hex[i];
            same = (c >= 'A' && c <= 'F' ? char(c - 'A' + 'a') : c) == known.sha256[i];
        }
        if (same) return true;
    }
    return false;
}

} // namespace mkwvk::dlss_nr
