// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helpers for classifying Vensim stdlib-call `#` signature names.
//!
//! Vensim emits stdlib-call output and internal-stock names into the VDF
//! name table using two incompatible encodings:
//!
//! | Style | Output sig form       | Internal stocks                     |
//! |-------|-----------------------|-------------------------------------|
//! | Old   | `#FUNCNAME(args)#`    | `#LV1<FUNCNAME(args)#`, `#DL<...`, ... |
//! | New   | `#alias>FUNC#`        | `#alias>FUNC>LV1#`, `#alias>FUNC>DL#`, ... |
//!
//! The new-style form encodes the user alias name directly in the prefix,
//! so user-alias-to-output-OT resolution is a pure split on `>`. The
//! old-style form leaves the alias name implicit and still requires a
//! parsed model to recover.
//!
//! The name table also contains `#`-bracketed names that are NOT stdlib
//! outputs -- internal Vensim helpers for macros like `RAMP FROM TO`
//! carry sub-part names (`#alias>RAMP FROM TO>linear#`, `>slope#`,
//! `>rate#`, ...) and some models emit bare `#var#` display names. Those
//! must not be counted as output signatures. The classifier therefore
//! REQUIRES a positive structural signal:
//! - old-style: the name contains `(` (the stdlib argument list), OR
//! - new-style: the name contains exactly one `>` at the top level
//!   (separating the alias prefix from the function name).
//!
//! See `docs/design/vdf.md` for the validation data behind these encodings.

/// Old-style internal `#` signature prefixes. Names starting with one of
/// these are stdlib internal stocks/rates (e.g. `#LV1<DELAY1(...)>`) that
/// back a module but do not correspond to a user-facing alias output.
pub(crate) const INTERNAL_OLD_SIG_PREFIXES: [&str; 7] =
    ["#LV1<", "#LV2<", "#LV3<", "#ST<", "#DL<", "#RT1<", "#RT2<"];

/// New-style internal `#` signature suffixes. Names ending with one of
/// these are stdlib internal stocks/rates in the newer Vensim
/// `#alias>FUNC>STOCK#` encoding (e.g. `#defaults>DELAY1>LV1#`).
pub(crate) const INTERNAL_NEW_SIG_SUFFIXES: [&str; 7] =
    [">LV1#", ">LV2#", ">LV3#", ">ST#", ">DL#", ">RT1#", ">RT2#"];

/// Whether a `#` signature is the *output* of a stdlib call -- the name a
/// user alias would bind to -- rather than one of the module's internal
/// stocks or rates.
///
/// Non-`#` names always return `false`. The predicate requires a positive
/// structural indicator (a `(` for old-style, exactly one `>` for new-style)
/// so non-stdlib `#`-bracketed names like `#BAU atm conc CO2#` or a bare
/// `#` do not pass:
/// - **Old style** (`#FUNCNAME(args)#`): the alias name is not part of the
///   signature; internal helpers are emitted as
///   `#LV1<FUNCNAME(args)#`, `#DL<...`, etc. Requires `(` somewhere in
///   the name.
/// - **New style** (`#alias>FUNC#`): the alias name sits in the prefix
///   and a trailing `>LV1`, `>DL`, ... tags an internal stock/rate.
///   Requires exactly ONE `>` at the top level; names with 2+ `>` are
///   sub-parts of multi-output macros like `RAMP FROM TO`
///   (`#alias>RAMP FROM TO>linear#`, `>slope#`, `>rate#`, ...) and are
///   rejected.
pub(crate) fn is_output_sig_name(name: &str) -> bool {
    if !name.starts_with('#') || !name.ends_with('#') || name.len() < 3 {
        return false;
    }
    // Reject old-style internal prefix markers first.
    if INTERNAL_OLD_SIG_PREFIXES
        .iter()
        .any(|p| name.starts_with(p))
    {
        return false;
    }
    // Reject new-style internal suffix markers.
    if INTERNAL_NEW_SIG_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return false;
    }
    let inner = &name[1..name.len() - 1];
    let has_paren = inner.contains('(');
    let gt_count = inner.matches('>').count();
    // Old-style: `#FUNCNAME(args)#` contains `(`.
    if has_paren {
        return true;
    }
    // New-style: `#alias>FUNC#` has exactly one `>`. Multi-`>` names are
    // RAMP-FROM-TO-style sub-parts which are internal helpers, not
    // user-alias outputs. Zero-`>` names (e.g. `#MP RF Total#`) are
    // Vensim display names without a stdlib call.
    gt_count == 1
}

/// Parse the user-alias name out of a new-style `#alias>FUNC#` signature.
///
/// Returns `None` for old-style `#FUNC(args)#` signatures (which do not
/// encode the alias name) and for malformed inputs.
pub(crate) fn parse_new_style_alias_sig(sig: &str) -> Option<&str> {
    if !sig.starts_with('#') || !sig.ends_with('#') || sig.len() < 3 {
        return None;
    }
    let inner = &sig[1..sig.len() - 1];
    let (alias, _rest) = inner.split_once('>')?;
    Some(alias)
}

/// Return new-style stdlib-call signature triples in name-table file order.
///
/// Each triple is `(name_idx, signature_name, alias_name)`. The alias is
/// parsed directly out of the `#alias>FUNC#` encoding -- so this function
/// only returns entries for fixtures that use the newer Vensim signature
/// form.
///
/// Only the *canonical* top-level `#alias>FUNC#` form is emitted. The
/// multi-`>` sub-part names that Vensim writes for stateful macros
/// (`#alias>RAMP FROM TO>linear#`, `>slope#`, `>rate#`, `>interval#`, ...)
/// are filtered out by [`is_output_sig_name`]; this keeps the alias list
/// 1:1 with the user-facing alias set rather than 7:1-inflated per RAMP
/// alias.
///
/// Old-style fixtures (`#FUNC(args)#`) yield an empty vector here because
/// the alias name is not encoded in the signature; recovering the alias
/// binding there requires the parsed model.
pub(crate) fn new_style_alias_signatures(names: &[String]) -> Vec<(usize, String, String)> {
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if !is_output_sig_name(name) {
            continue;
        }
        if let Some(alias) = parse_new_style_alias_sig(name) {
            out.push((i, name.clone(), alias.to_string()));
        }
    }
    out
}

/// Return all output-type `#` signature names in name-table file order.
///
/// Output signatures are the names that a user alias may bind to: the
/// top-level function result (`#DELAY1(...)`, `#SMOOTH(...)`, or the
/// new-style `#alias>FUNC#`). Internal stdlib stocks and rates
/// (`#LV1<...>`, `#RT1<...>`, `#alias>FUNC>LV1#`, etc.) are excluded.
pub(crate) fn output_signatures(names: &[String]) -> Vec<(usize, String)> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| is_output_sig_name(n))
        .map(|(i, n)| (i, n.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_output_sig_recognizes_old_style_outputs_and_internals() {
        // Old-style outputs -- contain `(`.
        assert!(is_output_sig_name("#DELAY1(a,b)#"));
        assert!(is_output_sig_name("#SMOOTH(x,3)#"));
        assert!(is_output_sig_name("#SMOOTH3(arg1,arg2)#"));
        assert!(is_output_sig_name("#TREND(a,b)#"));
        // Old-style internal stocks/rates.
        assert!(!is_output_sig_name("#LV1<DELAY1(a,b)#"));
        assert!(!is_output_sig_name("#LV2<SMOOTH3(x,y)#"));
        assert!(!is_output_sig_name("#LV3<SMOOTH3(x,y)#"));
        assert!(!is_output_sig_name("#ST<SMOOTH(x,y)#"));
        assert!(!is_output_sig_name("#DL<DELAY3(x,y)#"));
        assert!(!is_output_sig_name("#RT1<DELAY3(x,y)#"));
        assert!(!is_output_sig_name("#RT2<DELAY3(x,y)#"));
    }

    #[test]
    fn test_is_output_sig_recognizes_new_style_outputs_and_internals() {
        // New-style outputs: exactly one `>`.
        assert!(is_output_sig_name("#defaults>DELAY1#"));
        assert!(is_output_sig_name("#perceived HPI>SMOOTH#"));
        assert!(is_output_sig_name("#perceived mortgage balance>SMOOTH#"));
        // New-style internals: named LV1/LV2/LV3/ST/DL/RT1/RT2 suffixes.
        assert!(!is_output_sig_name("#defaults>DELAY1>LV1#"));
        assert!(!is_output_sig_name("#perceived x>SMOOTH3>LV2#"));
        assert!(!is_output_sig_name("#anything>DELAY3>DL#"));
    }

    /// Negative: RAMP FROM TO sub-parts carry multi-`>` names that look
    /// new-style but are NOT user-alias outputs. All seven sub-parts per
    /// RAMP alias must be rejected so they do not polute the output sig
    /// count (Issue H/I).
    #[test]
    fn test_is_output_sig_rejects_ramp_from_to_subparts() {
        assert!(is_output_sig_name(
            "#Relative forestry emissions to target>RAMP FROM TO#"
        ));
        for sub in [
            "linear",
            "linear ramp",
            "exp ramp",
            "slope",
            "rate",
            "interval",
        ] {
            let name = format!("#Relative forestry emissions to target>RAMP FROM TO>{sub}#");
            assert!(
                !is_output_sig_name(&name),
                "expected {name:?} to be rejected as a multi-> sub-part"
            );
        }
    }

    /// Negative: SSHAPE, SAMPLE UNTIL, delay3 and similar new-style
    /// signatures must properly classify their internal parts. The
    /// `>input#` sub-part of SSHAPE is an internal helper.
    #[test]
    fn test_is_output_sig_rejects_multi_gt_subparts_beyond_ramp() {
        assert!(is_output_sig_name("#target realization s shape>SSHAPE#"));
        assert!(!is_output_sig_name(
            "#target realization s shape>SSHAPE>input#"
        ));
        assert!(is_output_sig_name(
            "#Global emissions with linear reduction>delay3#"
        ));
        // Internal delay3 stocks match the new-style suffix list.
        assert!(!is_output_sig_name(
            "#Global emissions with linear reduction>delay3>LV3#"
        ));
    }

    /// Negative: `#`-bracketed names that lack `(` and `>` entirely are
    /// display-only Vensim names, not stdlib outputs. Examples observed
    /// on Ref.vdf: `#BAU atm conc CO2#`, `#MP RF Total#`,
    /// `#Calculated Developed RS CO2eq#`.
    #[test]
    fn test_is_output_sig_rejects_hash_display_names_without_func_marker() {
        for name in [
            "#BAU atm conc CO2#",
            "#BAU atm conc CO2eq#",
            "#BAU temperature change from preindustrial#",
            "#Calculated Developed RS CO2eq#",
            "#Calculated Developing A RS CO2eq#",
            "#Calculated Developing B RS CO2eq#",
            "#Calculated Global RS CO2eq#",
            "#MP RF Total#",
            "#desired stock#",
            "#inline lookup table#",
        ] {
            assert!(
                !is_output_sig_name(name),
                "expected {name:?} (no `(` and no `>`) to be rejected"
            );
        }
    }

    /// Negative: a bare `#` from an empty-aux fixture must be rejected.
    #[test]
    fn test_is_output_sig_rejects_bare_hash() {
        assert!(!is_output_sig_name("#"));
        assert!(!is_output_sig_name("##"));
    }

    #[test]
    fn test_is_output_sig_rejects_non_signature_names() {
        assert!(!is_output_sig_name(""));
        assert!(!is_output_sig_name("defaults"));
        assert!(!is_output_sig_name("Time"));
        assert!(!is_output_sig_name(".Control"));
        assert!(!is_output_sig_name("-dmnl"));
    }

    #[test]
    fn test_parse_new_style_alias_extracts_prefix() {
        assert_eq!(
            parse_new_style_alias_sig("#defaults>DELAY1#"),
            Some("defaults")
        );
        assert_eq!(
            parse_new_style_alias_sig("#perceived HPI>SMOOTH#"),
            Some("perceived HPI")
        );
        // Internal stocks still have a `>`-prefix; they still parse, but the
        // caller filters them via is_output_sig_name first.
        assert_eq!(
            parse_new_style_alias_sig("#defaults>DELAY1>LV1#"),
            Some("defaults")
        );
    }

    #[test]
    fn test_parse_new_style_alias_rejects_old_style_and_malformed() {
        // Old-style signatures have no `>`.
        assert_eq!(parse_new_style_alias_sig("#DELAY1(a,b)#"), None);
        assert_eq!(parse_new_style_alias_sig("#SMOOTH(x,3)#"), None);
        // Malformed (missing leading/trailing `#`).
        assert_eq!(parse_new_style_alias_sig("defaults>DELAY1"), None);
        assert_eq!(parse_new_style_alias_sig("#defaults>DELAY1"), None);
        assert_eq!(parse_new_style_alias_sig("defaults>DELAY1#"), None);
        // Bare / empty.
        assert_eq!(parse_new_style_alias_sig("#"), None);
        assert_eq!(parse_new_style_alias_sig("##"), None);
    }
}
