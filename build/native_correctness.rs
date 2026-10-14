//! Correct the pinned native session in an OUT_DIR copy, without editing vendor.

use std::fs;
use std::path::{Path, PathBuf};

/// Produce the bundled C API copy compiled by the CMake project hook.
///
/// # Arguments
///
/// * `source_dir` - CrispASR checkout containing the pinned C API source.
/// * `out_dir` - Cargo build output directory receiving the corrected copy.
///
/// # Returns
///
/// The path of the corrected C API translation unit.
///
/// # Errors
///
/// Reports unreadable source, unsupported source anchors, or output write errors.
pub fn prepare(source_dir: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let source_path = source_dir.join("src/crispasr_c_api.cpp");
    let source = fs::read_to_string(&source_path)
        .map_err(|err| format!("read {}: {err}", source_path.display()))?;
    let patched = patch_source(&source)?;
    let output_dir = out_dir.join("native-correctness");
    fs::create_dir_all(&output_dir)
        .map_err(|err| format!("create {}: {err}", output_dir.display()))?;
    let output = output_dir.join("crispasr_c_api.cpp");
    if fs::read_to_string(&output).ok().as_deref() != Some(patched.as_str()) {
        fs::write(&output, patched).map_err(|err| format!("write {}: {err}", output.display()))?;
    }
    Ok(output)
}

/// Apply the small corrections to exactly the source version we reviewed.
///
/// An override/upgraded source with different anchors must be reviewed before
/// it can silently replace these correctness fixes.
///
/// # Arguments
///
/// * `source` - Complete original CrispASR C API translation unit.
///
/// # Returns
///
/// A corrected source copy using Parakit's tested native helpers.
///
/// # Errors
///
/// Reports missing or duplicate patch anchors when the input differs from the
/// reviewed native source.
///
/// # Panics
///
/// Does not panic.
pub fn patch_source(source: &str) -> Result<String, String> {
    // Git checkouts on Windows may use CRLF; native patch anchors use LF.
    let source = source.replace("\r\n", "\n");
    let mut source = replace_once(
        &source,
        "#include \"crispasr_session.h\"",
        "#include \"crispasr_session.h\"\n#include \"qwen_correctness.hpp\"",
        "correctness helper include",
    )?;

    let begin = source
        .find("static crispasr_session_result* transcribe_autochunk(")
        .ok_or("native correction anchor missing: transcribe_autochunk")?;
    let end = source[begin..]
        .find("// Applies the session's resident")
        .map(|end| begin + end)
        .ok_or("native correction anchor missing: autochunk end")?;
    let chunk = replace_once(
        &source[begin..end],
        CHUNK_START,
        CHUNK_REPLACEMENT,
        "chunk merger",
    )?;
    let chunk = replace_once(
        &chunk,
        "        delete part;\n    }\n    return merged;",
        "    });",
        "chunk merger end",
    )?;
    source.replace_range(begin..end, &chunk);

    let begin = source
        .find("    // mega-asr is handled here too")
        .ok_or("native correction anchor missing: Qwen session")?;
    let end = source[begin..]
        .find("#endif")
        .map(|end| begin + end)
        .ok_or("native correction anchor missing: Qwen session end")?;
    let qwen = replace_once(
        &source[begin..end],
        "        std::free(eos_arr);\n\n        const int last_off",
        "        std::free(eos_arr);\n        if (s->backend == \"qwen3\" && !parakit_asr::valid_eos(n_eos, eos_id, vocab)) {\n            fprintf(stderr, \"crispasr[qwen3]: invalid EOS tokenization (expected one in-vocabulary token)\\n\");\n            std::free(logits);\n            delete r;\n            return nullptr;\n        }\n\n        const int last_off",
        "Qwen EOS validation",
    )?;
    let qwen = replace_once(
        &qwen,
        "        std::free(logits);\n\n        // Detokenize",
        "        std::free(logits);\n        if (s->backend == \"qwen3\" && !parakit_asr::decode_completed(dec.tokens, eos_id, vocab, q3_max_new)) {\n            fprintf(stderr, \"crispasr[qwen3]: incomplete decode (%s, generated %zu of %d tokens); transcription discarded\\n\",\n                    dec.tokens.size() >= (size_t)q3_max_new ? \"generation limit reached without EOS\" : \"decoder stopped before EOS\",\n                    dec.tokens.size(), q3_max_new);\n            delete r;\n            return nullptr;\n        }\n\n        // Detokenize",
        "Qwen completion gate",
    )?;
    let qwen = replace_once(
        &qwen,
        DETOKENIZE,
        DETOKENIZE_REPLACEMENT,
        "Qwen metadata parser",
    )?;
    source.replace_range(begin..end, &qwen);
    Ok(source)
}

fn replace_once(
    source: &str,
    needle: &str,
    replacement: &str,
    label: &str,
) -> Result<String, String> {
    let count = source.matches(needle).count();
    if count != 1 {
        return Err(format!(
            "native correction anchor {label:?}: expected once, found {count}; review the CrispASR source override or upgrade"
        ));
    }
    Ok(source.replacen(needle, replacement, 1))
}

const CHUNK_START: &str = r#"    auto* merged = new crispasr_session_result();
    for (const auto& range : ranges) {
        const size_t b = range.first, e = range.second;
        crispasr_session_result* part = transcribe_single(s, pcm + b, (int)(e - b), language);
        if (!part)
            continue;"#;

const CHUNK_REPLACEMENT: &str = r#"    return parakit_asr::transcribe_chunks<crispasr_session_result>(
        ranges,
        [&](const auto& range) {
            const size_t b = range.first, e = range.second;
            return transcribe_single(s, pcm + b, (int)(e - b), language);
        },
        [&](auto& merged_result, auto& part_result, const auto& range) {
        auto* merged = &merged_result;
        auto* part = &part_result;
        const size_t b = range.first;"#;

const DETOKENIZE: &str = r#"        std::string transcript;
        std::vector<ca_token_record> toks;
        toks.reserve(dec.tokens.size());
        bool capture_language = false;
        for (size_t i = 0; i < dec.tokens.size(); i++) {
            const int32_t id = dec.tokens[i];
            if (id == eos_id)
                break;
            const char* raw_piece = qwen3_asr_token_text(s->qwen3_ctx, id);
            if (!raw_piece || !*raw_piece)
                continue;
            std::string raw = raw_piece;
            // Skip qwen3 special tokens and structured tags.
            if (raw.size() >= 2 && raw[0] == '<' && raw[1] == '|')
                continue;
            if (raw.size() >= 2 && raw[0] == '<' && raw.back() == '>')
                continue;
            if (raw.size() >= 5 && raw[0] == '[' && raw[1] == 'P' && raw[2] == 'A' && raw[3] == 'D')
                continue;
            std::string piece = gpt2_byte_decode(raw);
            if (!raon && piece == "language") {
                capture_language = true;
                continue;
            }
            if (capture_language) {
                capture_language = false; // language name eaten, no transcript contribution
                continue;
            }
            transcript += piece;
            ca_token_record tk;
            tk.text = piece;
            tk.t0 = -1;
            tk.t1 = -1;
            tk.p = (i < dec.probs.size()) ? dec.probs[i] : -1.0f;
            toks.push_back(std::move(tk));
        }"#;

const DETOKENIZE_REPLACEMENT: &str = r#"        std::vector<std::string> pieces(dec.tokens.size());
        for (size_t i = 0; i < dec.tokens.size(); i++) {
            if (dec.tokens[i] == eos_id)
                break;
            const char* raw = qwen3_asr_token_text(s->qwen3_ctx, dec.tokens[i]);
            if (raw)
                pieces[i] = gpt2_byte_decode(raw);
        }
        pieces = parakit_asr::transcript_pieces(std::move(pieces), !raon && assistant_prefill.empty());
        std::string transcript;
        std::vector<ca_token_record> toks;
        toks.reserve(pieces.size());
        for (size_t i = 0; i < pieces.size(); i++) {
            if (pieces[i].empty())
                continue;
            transcript += pieces[i];
            ca_token_record tk;
            tk.text = std::move(pieces[i]);
            tk.t0 = -1;
            tk.t1 = -1;
            tk.p = (i < dec.probs.size()) ? dec.probs[i] : -1.0f;
            toks.push_back(std::move(tk));
        }"#;
