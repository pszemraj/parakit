// Model-free and opt-in loaded-session checks of the patched native path.
#undef NDEBUG // These executable regression checks must also run in Release builds.
#include "qwen_correctness.hpp"
#include "core/greedy_decode.h"

#include <cassert>
#include <cstdlib>
#include <iostream>
#include <numeric>
#include <utility>

#ifdef PARAKIT_LOADED_QWEN_FAULTS
#include "crispasr_session.h"
#include "qwen3_asr.h"
#include "core/wav_reader.h"
#include <climits>
#include <cstring>
#endif

namespace {

struct ChunkResult {
    static int live;
    std::string text;
    ChunkResult() { ++live; }
    ~ChunkResult() { --live; }
};
int ChunkResult::live = 0;

void quiet_range_selection_is_gain_independent() {
    constexpr size_t window = 1600;
    const float amplitudes[] = {6e-6f, 1.2e-5f, 2.3e-5f, .001f, .002f, .003f, 6e-6f, 1.2e-5f, 2.3e-5f, 0};
    std::vector<float> pcm;
    for (float amplitude : amplitudes)
        pcm.insert(pcm.end(), window, amplitude);
    const std::vector<std::pair<size_t, size_t>> ranges = {
        {0, 3 * window}, {3 * window, 6 * window}, {6 * window, 9 * window}, {9 * window, 10 * window}};
    const std::vector<std::pair<size_t, size_t>> expected(ranges.begin(), ranges.begin() + 3);
    assert(audio_chunking::drop_speechless_ranges(pcm.data(), pcm.size(), ranges, window).size() == 1);
    assert(parakit_asr::qwen_speech_ranges(pcm.data(), pcm.size(), ranges, window) == expected);
    for (float& sample : pcm)
        sample *= 100;
    assert(parakit_asr::qwen_speech_ranges(pcm.data(), pcm.size(), ranges, window) == expected);
    for (float& sample : pcm)
        sample *= 1e-6f;
    assert(parakit_asr::qwen_speech_ranges(pcm.data(), pcm.size(), ranges, window) == expected);
    std::vector<float> noise(9 * window, .0002f);
    std::fill(noise.begin() + 3 * window, noise.begin() + 6 * window, .1f);
    const auto kept = parakit_asr::qwen_speech_ranges(noise.data(), noise.size(),
        {ranges[0], ranges[1], ranges[2]}, window);
    const std::vector<std::pair<size_t, size_t>> only_noise_free{ranges[1]};
    assert(kept == only_noise_free);
}

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

#ifndef PARAKIT_LOADED_QWEN_FAULTS
int main() {
    chunks_are_all_or_nothing();
    quiet_range_selection_is_gain_independent();
    real_decoder_failure_contract();
    metadata_preserves_spoken_text();
    std::cout << "native correctness checks passed\n";
}
#else
// Only the isolated test build's generated C API copy calls these wrappers.
// The real model, encoder, prefill, decoder and session API remain in use.
namespace parakit_test {
enum class Fault { None, Embed, Forward, EmptyEos, MultipleEos, InvalidEos, LaterChunk };
Fault fault = Fault::None;
int embeds = 0, forwards = 0, chunks = 0, completed_chunks = 0;
bool injected = false;

void reset(Fault value) {
    fault = value;
    embeds = forwards = chunks = completed_chunks = 0;
    injected = false;
}

void begin_chunk() { ++chunks; }
void end_chunk(bool completed) { completed_chunks += completed; }

float* embed(qwen3_asr_context* ctx, const int32_t* ids, int count) {
    ++embeds;
    if ((fault == Fault::Embed && embeds == 2) ||
        (fault == Fault::LaterChunk && chunks == 2)) {
        injected = true;
        return nullptr;
    }
    return qwen3_asr_embed_tokens(ctx, ids, count);
}

float* forward(qwen3_asr_context* ctx, const float* values, int count, int past, int* out_count, int* vocab) {
    if (++forwards == 2 && fault == Fault::Forward) {
        injected = true;
        return nullptr;
    }
    return qwen3_asr_run_llm_kv(ctx, values, count, past, out_count, vocab);
}

int32_t* tokenize(qwen3_asr_context* ctx, const char* text, int* count) {
    if (std::strcmp(text, "<|im_end|>") == 0 &&
        (fault == Fault::EmptyEos || fault == Fault::MultipleEos || fault == Fault::InvalidEos)) {
        injected = true;
        *count = fault == Fault::EmptyEos ? 0 : fault == Fault::MultipleEos ? 2 : 1;
        if (!*count)
            return nullptr;
        auto* ids = static_cast<int32_t*>(std::calloc(*count, sizeof(int32_t)));
        ids[0] = INT_MAX;
        return ids;
    }
    return qwen3_asr_tokenize(ctx, text, count);
}
} // namespace parakit_test

// The pinned public header intentionally forward-declares this versioned ABI.
struct crispasr_open_params_v1 {
    int abi_version, n_threads, use_gpu, verbosity, flash_attn, n_gpu_layers;
    int reserved[6];
};

std::string result_text(crispasr_session_result* result) {
    assert(result);
    std::string text;
    for (int i = 0; i < crispasr_session_result_n_segments(result); ++i)
        text += crispasr_session_result_segment_text(result, i);
    crispasr_session_result_free(result);
    assert(!text.empty());
    return text;
}

int main(int argc, char** argv) {
    assert(argc == 3);
    std::vector<float> pcm;
    int sample_rate = 0;
    assert(crispasr::core::read_wav_mono_pcm16(argv[2], pcm, sample_rate));
    assert(sample_rate == 16000 && pcm.size() == 11 * 16000);
    const crispasr_open_params_v1 options{2, 8, 0, 0, 1, -1, {0}};
    auto* session = crispasr_session_open_with_params(argv[1], "qwen3", &options);
    assert(session);
    assert(crispasr_session_set_source_language(session, "English") == 0);
    assert(crispasr_session_set_max_new_tokens(session, 512) == 0);
    auto transcribe = [&]() {
        return crispasr_session_transcribe(session, pcm.data(), static_cast<int>(pcm.size()));
    };
    const auto baseline = result_text(transcribe());
    for (auto fault : {parakit_test::Fault::Embed, parakit_test::Fault::Forward,
                       parakit_test::Fault::EmptyEos, parakit_test::Fault::MultipleEos,
                       parakit_test::Fault::InvalidEos}) {
        parakit_test::reset(fault);
        auto* failed = transcribe();
        assert(parakit_test::injected && !failed);
        parakit_test::reset(parakit_test::Fault::None);
        assert(result_text(transcribe()) == baseline);
        std::cout << "loaded Qwen fault and recovery passed: " << static_cast<int>(fault) << std::endl;
    }

    // Cross the native 30-second boundary with real speech in each slice.
    std::vector<float> long_pcm;
    for (int i = 0; i < 3; ++i)
        long_pcm.insert(long_pcm.end(), pcm.begin(), pcm.end());
    parakit_test::reset(parakit_test::Fault::LaterChunk);
    auto* failed = crispasr_session_transcribe(session, long_pcm.data(), static_cast<int>(long_pcm.size()));
    assert(parakit_test::injected && !failed);
    assert(parakit_test::chunks == 2 && parakit_test::completed_chunks == 1);
    parakit_test::reset(parakit_test::Fault::None);
    result_text(crispasr_session_transcribe(session, long_pcm.data(), static_cast<int>(long_pcm.size())));
    assert(parakit_test::chunks >= 2 && parakit_test::completed_chunks == parakit_test::chunks);
    assert(result_text(transcribe()) == baseline);
    crispasr_session_close(session);
    std::cout << "loaded Qwen chunk fault and recovery passed\n";
}
#endif
