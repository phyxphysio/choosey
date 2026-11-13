//! Input limits and cleaning shared by the web layer.

/// Maximum length of titles and names, in characters.
pub const MAX_NAME: usize = 100;
/// Maximum length of option notes, in characters.
pub const MAX_NOTES: usize = 1000;
/// Maximum length of a numeric criterion's unit, in characters.
pub const MAX_UNIT: usize = 30;
/// Maximum number of criteria per decision.
pub const MAX_CRITERIA: usize = 30;
/// Maximum number of options per decision.
pub const MAX_OPTIONS: usize = 50;
/// Largest magnitude accepted for a raw numeric value (keeps arithmetic well away from overflow).
pub const MAX_ABS_VALUE: f64 = 1e12;

/// A user input that we refuse, with a message safe to show back to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidInput(pub String);

fn invalid(msg: impl Into<String>) -> InvalidInput {
    InvalidInput(msg.into())
}

/// Clean a single-line text field: drop control characters (including newlines) and trim.
/// Fails if the result is empty or longer than `max` characters.
pub fn clean_line(field: &str, input: &str, max: usize) -> Result<String, InvalidInput> {
    let cleaned: String = input.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Err(invalid(format!("{field} can't be empty.")));
    }
    if cleaned.chars().count() > max {
        return Err(invalid(format!(
            "{field} must be at most {max} characters."
        )));
    }
    Ok(cleaned.to_string())
}

/// Like [`clean_line`] but an empty result is allowed.
pub fn clean_optional_line(field: &str, input: &str, max: usize) -> Result<String, InvalidInput> {
    if input.trim().is_empty() {
        return Ok(String::new());
    }
    clean_line(field, input, max)
}

/// Clean a multi-line text field: keep newlines (normalised to `\n`), drop other control
/// characters, trim. Empty is allowed.
pub fn clean_text(field: &str, input: &str, max: usize) -> Result<String, InvalidInput> {
    let cleaned: String = input
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || !c.is_control())
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.chars().count() > max {
        return Err(invalid(format!(
            "{field} must be at most {max} characters."
        )));
    }
    Ok(cleaned.to_string())
}

/// Parse an optional number. Empty means "no value". Rejects NaN, infinities and absurd magnitudes.
pub fn parse_number(field: &str, input: &str) -> Result<Option<f64>, InvalidInput> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let x: f64 = s
        .parse()
        .map_err(|_| invalid(format!("{field}: \"{}\" is not a number.", truncate(s, 20))))?;
    if !x.is_finite() || x.abs() > MAX_ABS_VALUE {
        return Err(invalid(format!("{field}: number out of range.")));
    }
    Ok(Some(x))
}

/// Parse an optional 0-10 rating in half steps.
pub fn parse_rating(field: &str, input: &str) -> Result<Option<f64>, InvalidInput> {
    match parse_number(field, input)? {
        None => Ok(None),
        Some(x) if (0.0..=10.0).contains(&x) && (x * 2.0).fract() == 0.0 => Ok(Some(x)),
        Some(_) => Err(invalid(format!(
            "{field}: ratings go from 0 to 10 in half steps."
        ))),
    }
}

/// Parse a 0-10 integer weight.
pub fn parse_weight(field: &str, input: &str) -> Result<u8, InvalidInput> {
    match input.trim().parse::<u8>() {
        Ok(w) if w <= 10 => Ok(w),
        _ => Err(invalid(format!(
            "{field}: weight must be a whole number from 0 to 10."
        ))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_cleaned() {
        assert_eq!(clean_line("Name", "  a\u{0}b\nc\t ", 10).unwrap(), "abc");
        assert!(clean_line("Name", " \n ", 10).is_err());
        assert!(clean_line("Name", &"é".repeat(11), 10).is_err());
        assert_eq!(
            clean_line("Name", &"é".repeat(10), 10)
                .unwrap()
                .chars()
                .count(),
            10
        );
        assert_eq!(clean_optional_line("Unit", "  ", 10).unwrap(), "");
    }

    #[test]
    fn text_keeps_newlines() {
        assert_eq!(clean_text("Notes", " a\r\nb\u{7}\n", 10).unwrap(), "a\nb");
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("x", "").unwrap(), None);
        assert_eq!(parse_number("x", " 12.5 ").unwrap(), Some(12.5));
        assert_eq!(parse_number("x", "-3").unwrap(), Some(-3.0));
        assert!(parse_number("x", "NaN").is_err());
        assert!(parse_number("x", "inf").is_err());
        assert!(parse_number("x", "1e300").is_err());
        assert!(parse_number("x", "abc").is_err());
    }

    #[test]
    fn ratings_and_weights() {
        assert_eq!(parse_rating("x", "7.5").unwrap(), Some(7.5));
        assert_eq!(parse_rating("x", "0").unwrap(), Some(0.0));
        assert!(parse_rating("x", "7.3").is_err());
        assert!(parse_rating("x", "11").is_err());
        assert!(parse_rating("x", "-1").is_err());
        assert_eq!(parse_weight("w", "10").unwrap(), 10);
        assert!(parse_weight("w", "11").is_err());
        assert!(parse_weight("w", "-1").is_err());
        assert!(parse_weight("w", "2.5").is_err());
    }
}
