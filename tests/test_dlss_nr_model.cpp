// Which file runs DLSS 5 (gpu/vk/dlss_nr_model.h), its SHA-256 and the FP16 allow-list: tests/test_dlss_nr_model.py.
#include "../gpu/vk/dlss_nr_model.h"
#include <algorithm>
#include <cassert>
#include <cstdio>
#include <string>
using namespace mkwvk::dlss_nr;

static std::string sha(const std::string& s) {
    Sha256 h;
    h.update(s.data(), s.size());
    return h.hex();
}

int main() {
    // the choice table: every GPU (RTX 50 or not), every pair of files, every choice of the row
    for (int choice : {(int)kModelAuto, (int)kModelNvidia, (int)kModelFp16, 7}) {
        for (bool rtx50 : {false, true}) {
            assert(pick(rtx50, {false, false}, choice) == Model::none);            // (no file: no DLSS 5 rows at all)
            assert(pick(rtx50, {true, false}, choice) == Model::nvidia);           // (NVIDIA's alone: NGX's path, as before)
            assert(pick(rtx50, {false, true}, choice) == Model::fp16);             // (the FP16 file alone, whatever the GPU)
            assert(!model_row(rtx50, {false, false}) && !model_row(rtx50, {true, false}) && !model_row(rtx50, {false, true}));
        }
        // both files: RTX 20, 30 and 40 run the FP16 one (NVIDIA's FP8 model does not run there), with no row
        assert(pick(false, {true, true}, choice) == Model::fp16 && !model_row(false, {true, true}));
        // an RTX 50 with both: the row, and its choice (Auto is NVIDIA's; a value past the row's is Auto too)
        assert(model_row(true, {true, true}));
        assert(pick(true, {true, true}, choice) == (choice == kModelFp16 ? Model::fp16 : Model::nvidia));
    }
    // SHA-256: FIPS 180-4's examples, and the same digest however the bytes are split
    assert(sha("") == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert(sha("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert(sha("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq") ==
           "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    {
        Sha256 h;
        const std::string a(1000, 'a');
        for (int i = 0; i < 1000; i++) h.update(a.data(), a.size());   // (a million 'a')
        assert(h.hex() == "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }
    {
        std::string text;
        for (int i = 0; i < 300; i++) text += char('a' + i % 26) + std::to_string(i);
        const std::string whole = sha(text);
        for (size_t step : {1u, 3u, 63u, 64u, 65u, 127u, 200u}) {
            Sha256 h;
            for (size_t at = 0; at < text.size(); at += step) h.update(text.data() + at, std::min(step, text.size() - at));
            assert(h.hex() == whole);
        }
    }
    // the allow-list: the Bloodborne port's known file, in either case; nothing else
    assert(sizeof(kFp16Known) / sizeof(kFp16Known[0]) == 1 && kFp16Known[0].size == 276162560ull);
    assert(fp16_size_known(276162560ull) && !fp16_size_known(276162561ull) && !fp16_size_known(266424944ull) &&
           !fp16_size_known(0));   // (NVIDIA's own file is 266,424,944 bytes: not even read)
    assert(fp16_known("cdfa6c1ec784867182164c7836f324f087b5c9f374d0c9fe46f54999feabf3ed"));
    assert(fp16_known("CDFA6C1EC784867182164C7836F324F087B5C9F374D0C9FE46F54999FEABF3ED"));
    assert(!fp16_known("cdfa6c1ec784867182164c7836f324f087b5c9f374d0c9fe46f54999feabf3ee"));
    assert(!fp16_known("cdfa6c1ec784867182164c7836f324f087b5c9f374d0c9fe46f54999feabf3e"));
    assert(!fp16_known(sha("abc")) && !fp16_known(""));
    std::puts("DLSS 5's file choice, SHA-256 and the FP16 allow-list: PASS");
}
