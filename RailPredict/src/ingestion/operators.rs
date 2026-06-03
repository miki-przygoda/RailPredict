//! Curated brand colours for GB train operating companies.
//!
//! Operator *names* come from the GTFS `agency.txt` feed; this module attaches a
//! brand colour by matching on a normalised name. Unknown operators fall back to
//! a neutral grey. Colours are hex strings suitable for CSS / SVG fills.

/// Neutral fallback for operators we don't have a brand colour for.
pub const DEFAULT_BRAND: &str = "#9aa7b4";

/// Return a brand colour for an operator, matched case-insensitively on a
/// substring of its name. Falls back to [`DEFAULT_BRAND`].
pub fn brand_color(operator_name: &str) -> &'static str {
    let n = operator_name.to_ascii_lowercase();
    const TABLE: &[(&str, &str)] = &[
        ("great western", "#0a493e"),
        ("avanti", "#11354e"),
        ("south western", "#24398c"),
        ("southeastern", "#00a3e0"),
        ("southern", "#8cc63f"),
        ("thameslink", "#ff5aa7"),
        ("great northern", "#1d1d4e"),
        ("gatwick express", "#ec1c24"),
        ("c2c", "#b7007c"),
        ("chiltern", "#00bfff"),
        ("cross country", "#660f21"),
        ("crosscountry", "#660f21"),
        ("east midlands", "#713563"),
        ("greater anglia", "#d70428"),
        ("hull trains", "#1d1d1b"),
        ("lumo", "#2b2e83"),
        ("london north eastern", "#ce0e2d"),
        ("lner", "#ce0e2d"),
        ("london overground", "#ee7d11"),
        ("merseyrail", "#fdb913"),
        ("northern", "#262262"),
        ("scotrail", "#1e3a8a"),
        ("transpennine", "#0a0a64"),
        ("transport for wales", "#ee2e24"),
        ("west midlands", "#e07e26"),
        ("elizabeth line", "#7156a5"),
        ("heathrow express", "#532e63"),
        ("grand central", "#1d1d1b"),
        ("caledonian sleeper", "#1b1b3a"),
        ("island line", "#1e90ff"),
        ("stansted express", "#6a1b9a"),
    ];
    for (needle, hex) in TABLE {
        if n.contains(needle) {
            return hex;
        }
    }
    DEFAULT_BRAND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_operator_gets_brand() {
        assert_eq!(brand_color("Great Western Railway"), "#0a493e");
        assert_eq!(brand_color("AVANTI WEST COAST"), "#11354e");
        assert_eq!(brand_color("London North Eastern Railway"), "#ce0e2d");
    }

    #[test]
    fn unknown_operator_falls_back() {
        assert_eq!(brand_color("Imaginary Trains Ltd"), DEFAULT_BRAND);
        assert_eq!(brand_color(""), DEFAULT_BRAND);
    }
}
