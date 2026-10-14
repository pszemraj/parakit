// Model-free checks of the helpers used by the patched native session.
#include "qwen_correctness.hpp"
#include "core/greedy_decode.h"

#include <cassert>
#include <cstdlib>
#include <iostream>
#include <numeric>
#include <utility>

namespace {

struct ChunkResult {
    static int live;
    std::string text;
    ChunkResult() { ++live; }
    ~ChunkResult() { --live; }
};
int ChunkResult::live = 0;

void chunks_are_all_or_nothing() {
    const std::vector<int> ranges{0, 1, 2};
    for (int fail : {-1, 0, 1, 2}) {
        int calls = 0;
        std::unique_ptr<ChunkResult> result(parakit_asr::transcribe_chunks<ChunkResult>(
            ranges,
            [&](int index) -> ChunkResult* {
                ++calls;
                if (index == fail)
                    return nullptr;
                auto* part = new ChunkResult();
                part->text = std::to_string(index);
                return part;
            },
            [](ChunkResult& merged, ChunkResult& part, int) { merged.text += part.text; }));
        if (fail < 0) {
            assert(result && result->text == "012");
            assert(calls == 3 && ChunkResult::live == 1);
        } else {
            assert(!result && calls == fail + 1 && ChunkResult::live == 0);
        }
    }
    assert(ChunkResult::live == 0);
}

struct Decoder {
    int calls = 0;
    bool embed_failure = false;
    bool forward_failure = false;
    bool emit_eos = true;
};

float* embed(Decoder* decoder, const int32_t*, int) {
    if (decoder->embed_failure)
        return nullptr;
    return static_cast<float*>(std::calloc(1, sizeof(float)));
}

float* forward(Decoder* decoder, const float*, int, int, int*, int*) {
    if (decoder->forward_failure)
        return nullptr;
    auto* logits = static_cast<float*>(std::calloc(8, sizeof(float)));
    logits[decoder->emit_eos && ++decoder->calls >= 2 ? 5 : 3] = 10.0f;
    return logits;
}

void real_decoder_failure_contract() {
    core_greedy_decode::Config config;
    config.max_new_tokens = 4;
    config.eos_id = 5;
    config.vocab_size = 8;
    for (int failure : {-1, 0, 1, 2}) {
        Decoder decoder;
        decoder.embed_failure = failure == 0;
        decoder.forward_failure = failure == 1;
        decoder.emit_eos = failure != 2;
        auto result = core_greedy_decode::run_with_probs(&decoder, 3, 1.0f, 0, embed, forward, config);
        const bool complete = parakit_asr::decode_completed(result.tokens, 5, 8, config.max_new_tokens);
        assert(complete == (failure < 0));
    }
    Decoder decoder;
    auto eos_only = core_greedy_decode::run_with_probs(&decoder, 5, 1.0f, 0, embed, forward, config);
    assert(parakit_asr::decode_completed(eos_only.tokens, 5, 8, 4));
    assert(decoder.calls == 0);
    assert(parakit_asr::decode_completed({3, 3, 3, 5}, 5, 8, 4));
    assert(parakit_asr::decode_completed({5}, 5, 8, 1));
    assert(!parakit_asr::decode_completed({3, 3, 3, 3}, 5, 8, 4));
    assert(!parakit_asr::decode_completed({3}, 5, 8, 4));
    assert(!parakit_asr::decode_completed({}, 5, 8, 4));
    assert(!parakit_asr::decode_completed({5}, -1, 8, 4));
    assert(!parakit_asr::decode_completed({5}, 8, 8, 4));
    assert(!parakit_asr::decode_completed({3, 5}, 5, 8, 1));
    assert(!parakit_asr::decode_completed({5}, 5, 8, 0));
    assert(parakit_asr::valid_eos(1, 5, 8));
    assert(!parakit_asr::valid_eos(0, 5, 8));
    assert(!parakit_asr::valid_eos(2, 5, 8));
    assert(!parakit_asr::valid_eos(1, -1, 8));
    assert(!parakit_asr::valid_eos(1, 8, 8));
}

std::string join(const std::vector<std::string>& pieces) {
    return std::accumulate(pieces.begin(), pieces.end(), std::string());
}

void metadata_preserves_spoken_text() {
    using parakit_asr::transcript_pieces;
    auto automatic = transcript_pieces({"language", " English", "<asr_text>", "language", " matters"}, true);
    assert(join(automatic) == "language matters");
    assert(automatic.size() == 5 && automatic[3] == "language");
    assert(join(transcript_pieces({"language", " matters"}, false)) == "language matters");
    assert(join(transcript_pieces({"language", " matters"}, true)) == "language matters");
    assert(join(transcript_pieces({"<asr_text>", "language", " matters"}, true)) == "language matters");
    assert(join(transcript_pieces({"This", " language", " matters"}, true)) == "This language matters");
    assert(join(transcript_pieces({"language", " English", "<asr_text>", "bonjour ", "世界"}, true)) == "bonjour 世界");
    assert(join(transcript_pieces({"language", " Canton", "ese", "<asr_text>", "廣東話"}, true)) == "廣東話");
    assert(join(transcript_pieces({"language English<asr_text>世界"}, true)) == "世界");
    assert(join(transcript_pieces({"language English", "<asr_", "text>", "世界"}, true)) == "世界");
    assert(join(transcript_pieces({"<asr_", "text>", "language", " matters"}, false)) == "language matters");
    assert(join(transcript_pieces({"<asr_", "text>language", " matters"}, true)) == "language matters");
    assert(join(transcript_pieces({"<|im_start|>", "language English", "<asr_text>", "hello", "[PAD0]", "<punc>"}, true)) == "hello");
    assert(join(transcript_pieces({"language none", "<asr_text>"}, true)).empty());
    assert(join(transcript_pieces({"language none", "<asr_text>", "<non_speech>"}, true)).empty());
    assert(join(transcript_pieces({"<asr_text>"}, false)).empty());
    assert(join(transcript_pieces({}, true)).empty());
}

} // namespace

int main() {
    chunks_are_all_or_nothing();
    real_decoder_failure_contract();
    metadata_preserves_spoken_text();
    std::cout << "native correctness checks passed\n";
}
