//! CSV export of a decision: one row per option, ranked, with raw values and 0-10 scores.

use choosey_core::model::{CriterionKind, Decision};
use choosey_core::scoring;

use crate::views::fmt1;

/// Neutralise spreadsheet formula injection: a text cell starting with `=`, `+`, `-`, `@`, tab or
/// carriage return could be run as a formula when the CSV is opened in Excel or Sheets, so prefix
/// it with a single quote. Only applied to user-entered text, never to our own numbers.
fn text_cell(s: &str) -> String {
    match s.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => format!("'{s}"),
        _ => s.to_string(),
    }
}

/// Build the CSV document. The `csv` crate handles quoting of commas, quotes and newlines.
pub fn to_csv(d: &Decision) -> Result<Vec<u8>, csv::Error> {
    let sheet = d.owner_scores();
    let mut w = csv::Writer::from_writer(Vec::new());

    let mut header = vec![
        "Rank".to_string(),
        "Option".to_string(),
        "Notes".to_string(),
        "Total (0-100)".to_string(),
        "Coverage (%)".to_string(),
    ];
    for c in &d.criteria {
        let unit = match &c.kind {
            CriterionKind::Numeric { unit, .. } if !unit.is_empty() => format!(" [{unit}]"),
            _ => String::new(),
        };
        header.push(text_cell(&format!(
            "{}{unit} (weight {}) value",
            c.name, c.weight
        )));
        header.push(text_cell(&format!("{} score (0-10)", c.name)));
    }
    w.write_record(&header)?;

    // If results can't be computed (no criteria, all weights 0) still export the options.
    let ranked = scoring::compute(&d.criteria, &d.options, sheet)
        .map(|r| r.ranked)
        .unwrap_or_default();
    let order: Vec<(Option<usize>, &choosey_core::model::OptionItem)> = if ranked.is_empty() {
        d.options.iter().map(|o| (None, o)).collect()
    } else {
        ranked
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                d.options
                    .iter()
                    .find(|o| o.id == r.option_id)
                    .map(|o| (Some(i), o))
            })
            .collect()
    };

    for (rank, option) in order {
        let result = rank.map(|i| &ranked[i]);
        let mut row = vec![
            rank.map(|i| (i + 1).to_string()).unwrap_or_default(),
            text_cell(&option.name),
            text_cell(&option.notes),
            result.and_then(|r| r.total).map(fmt1).unwrap_or_default(),
            result
                .map(|r| format!("{:.0}", r.coverage * 100.0))
                .unwrap_or_default(),
        ];
        for (ci, c) in d.criteria.iter().enumerate() {
            row.push(
                sheet
                    .get(&option.id)
                    .and_then(|m| m.get(&c.id))
                    .map(|x| x.to_string())
                    .unwrap_or_default(),
            );
            row.push(
                result
                    .and_then(|r| r.per_criterion[ci].score)
                    .map(fmt1)
                    .unwrap_or_default(),
            );
        }
        w.write_record(&row)?;
    }
    w.into_inner().map_err(|e| e.into_error().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use choosey_core::model::CriterionKind;

    #[test]
    fn exports_and_escapes() {
        let mut d = Decision::new("v".into(), "h".into(), "t".into(), 0);
        let c = d.add_criterion("Light, \"sun\"".into(), CriterionKind::Rated, 5);
        let o = d.add_option("=HYPERLINK(\"x\")".into(), "line1\nline2".into());
        d.owner_scores_mut().entry(o).or_default().insert(c, 7.5);
        let csv = String::from_utf8(to_csv(&d).unwrap()).unwrap();
        assert!(
            csv.contains("\"Light, \"\"sun\"\" (weight 5) value\""),
            "{csv}"
        );
        assert!(csv.contains("\"'=HYPERLINK(\"\"x\"\")\""), "{csv}");
        assert!(csv.contains("\"line1\nline2\""));
        assert!(csv.contains(",7.5,7.5"), "{csv}");
        assert!(csv.starts_with("Rank,Option"));
    }
}
