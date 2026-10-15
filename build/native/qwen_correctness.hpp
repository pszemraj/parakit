#pragma once

// Local session corrections for the pinned CrispASR C API. The bundled build
// includes this header from its patched build-directory source copy.
#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <string>
#include <vector>
#include "core/audio_chunking.h"

namespace parakit_asr {

// Exclude actual zero padding when estimating the relative noise floor. The
// pinned 1e-5 cutoff also excluded quiet nonzero windows, raising the estimate
// and dropping spoken clauses when the whole recording had low absolute gain.
// Keep speech above twice that floor: the pinned four-times threshold also
// discarded recoverable low-level PCM16 clauses around a louder speaker.
inline std::vector<std::pair<size_t, size_t>> qwen_speech_ranges(
    const float* pcm, size_t count, const std::vector<std::pair<size_t, size_t>>& ranges,
    size_t window) {
    if (ranges.size() <= 1)
        return ranges;
    const auto levels = audio_chunking::estimate_energy_levels(
        pcm, count, window, std::numeric_limits<float>::min());
    const float threshold = std::min(2.0f * levels.floor,
        levels.floor + 0.2f * (levels.loud - levels.floor));
    std::vector<std::pair<size_t, size_t>> kept;
    for (const auto& range : ranges) {
        float peak = 0;
        for (float rms : audio_chunking::window_rms_list(pcm + range.first, range.second - range.first, window))
            peak = std::max(peak, rms);
        if (peak > threshold)
            kept.push_back(range);
    }
    return kept;
}

// A failed slice invalidates the complete recording. Owning both results here
// also discards accumulated text when a later slice fails.
template <typename Result, typename Ranges, typename Transcribe, typename Append>
Result* transcribe_chunks(const Ranges& ranges, Transcribe transcribe, Append append) {
    auto merged = std::make_unique<Result>();
    for (const auto& range : ranges) {
        std::unique_ptr<Result> part(transcribe(range));
        if (!part)
            return nullptr;
        append(*merged, *part, range);
    }
    return merged.release();
}

// Greedy/beam helpers return their partial token vector on failure or budget
// exhaustion. Only a real EOS makes that vector a completed transcription.
inline bool valid_eos(int token_count, int eos, int vocab) {
    return token_count == 1 && eos >= 0 && eos < vocab;
}

inline bool decode_completed(const std::vector<int32_t>& tokens, int eos, int vocab, int budget) {
    if (eos < 0 || eos >= vocab || budget <= 0 || tokens.empty() ||
        tokens.size() > static_cast<size_t>(budget))
        return false;
    return tokens.back() == eos;
}

inline bool control_piece(const std::string& piece) {
    if (piece == "<asr_text>")
        return false; // Keep the delimiter until the metadata boundary is known.
    return (piece.size() >= 2 && piece.front() == '<' && piece.back() == '>') ||
           (piece.size() >= 5 && piece.compare(0, 4, "[PAD") == 0);
}

// Preserve the input token positions so callers retain confidence alignment.
// A language header is metadata only when followed by the ASR delimiter; the
// spoken word "language" elsewhere, including forced-language output, survives.
inline std::vector<std::string> transcript_pieces(std::vector<std::string> pieces, bool parse_language_header) {
    std::string joined;
    for (auto& piece : pieces) {
        if (control_piece(piece))
            piece.clear();
        joined += piece;
    }
    constexpr const char* tag = "<asr_text>";
    constexpr size_t tag_size = 10;
    const size_t delimiter = joined.find(tag);
    const size_t first = joined.find_first_not_of(" \t\r\n");
    size_t start = 0;
    if (parse_language_header && delimiter != std::string::npos && first != std::string::npos &&
        first < delimiter && joined.compare(first, 9, "language ") == 0)
        start = delimiter + tag_size;

    size_t offset = 0;
    for (auto& piece : pieces) {
        const size_t end = offset + piece.size();
        if (end <= start)
            piece.clear();
        else if (offset < start)
            piece.erase(0, start - offset);
        const size_t begin = std::max(offset, start);
        if (delimiter != std::string::npos && delimiter < end && delimiter + tag_size > begin) {
            const size_t tag_begin = std::max(delimiter, begin);
            const size_t tag_end = std::min(delimiter + tag_size, end);
            piece.erase(tag_begin - begin, tag_end - tag_begin);
        }
        offset = end;
        if (piece == tag)
            piece.clear();
    }
    return pieces;
}

} // namespace parakit_asr
