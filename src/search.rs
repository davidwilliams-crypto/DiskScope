//! Bounded, metadata-only investigation queries. Display paths are searchable
//! text; they are never converted back into filesystem/action targets.
use std::borrow::Cow;

pub const MAX_QUERY_CHARS: usize = 1024;
pub const MAX_QUERY_TERMS: usize = 32;
pub const SEARCH_HELP: &str = "Combine words to match all; use quotes for a phrase and - to exclude. Fields: name:, path:, ext:zip,7z and size:>=1GiB. Sizes use logical bytes; KB/MB/GB are decimal, KiB/MiB/GiB are binary. Search reads scanned metadata only. Limits: 1024 characters, 32 terms.";

#[derive(Debug)]
enum Predicate {
    Size(Comparison, u64),
    Extension(Vec<String>),
    Name(String),
    Path(String),
}
#[derive(Debug)]
struct Term {
    exclude: bool,
    predicate: Predicate,
}
#[derive(Clone, Copy, Debug)]
enum Comparison {
    Equal,
    Greater,
    AtLeast,
    Less,
    AtMost,
}

#[derive(Debug, Default)]
pub struct SearchQuery {
    terms: Vec<Term>,
}
impl SearchQuery {
    pub fn parse(input: &str) -> Result<Self, String> {
        if input.chars().take(MAX_QUERY_CHARS + 1).count() > MAX_QUERY_CHARS {
            return Err("Use at most 1024 search characters.".into());
        }
        let mut terms = Vec::new();
        let mut chars = input.chars().peekable();
        while chars.peek().is_some() {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
            if terms.len() == MAX_QUERY_TERMS {
                return Err("Use at most 32 search terms.".into());
            }
            // A quoted leading minus is literal. Backslashes stay literal so
            // Windows paths do not require escaping in this search field.
            let quoted_start = chars.peek() == Some(&'"');
            let mut quoted = false;
            let mut token = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    quoted = !quoted;
                } else if c.is_whitespace() && !quoted {
                    break;
                } else {
                    token.push(c);
                }
            }
            if quoted {
                return Err("Close the quoted search phrase.".into());
            }
            if token.is_empty() {
                return Err("Enter text inside the quotes.".into());
            }
            let exclude = !quoted_start && token.starts_with('-') && token.len() > 1;
            let text = if exclude { &token[1..] } else { &token }.to_lowercase();
            let (field, value) = text.split_once(':').unwrap_or(("", &text));
            let predicate = match field {
                "name" | "path" | "ext" | "size" if value.is_empty() => {
                    return Err(format!("Enter a value after {field}:."));
                }
                "name" => Predicate::Name(value.into()),
                "path" => Predicate::Path(value.replace('\\', "/")),
                "ext" => {
                    let extensions = value
                        .split(',')
                        .map(|ext| {
                            if ext == "(no extension)" {
                                return Ok(ext.into());
                            }
                            let ext = ext
                                .strip_prefix("*.")
                                .unwrap_or(ext)
                                .trim_start_matches('.');
                            if ext.is_empty() || ext.contains(['/', '\\', ' ']) {
                                Err("Use extensions such as ext:zip,7z.".to_owned())
                            } else {
                                Ok(format!(".{ext}"))
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Predicate::Extension(extensions)
                }
                "size" => {
                    let (comparison, number) = if let Some(n) = value.strip_prefix(">=") {
                        (Comparison::AtLeast, n)
                    } else if let Some(n) = value.strip_prefix("<=") {
                        (Comparison::AtMost, n)
                    } else if let Some(n) = value.strip_prefix('>') {
                        (Comparison::Greater, n)
                    } else if let Some(n) = value.strip_prefix('<') {
                        (Comparison::Less, n)
                    } else {
                        (Comparison::Equal, value.strip_prefix('=').unwrap_or(value))
                    };
                    Predicate::Size(comparison, parse_bytes(number)?)
                }
                _ => Predicate::Name(text),
            };
            terms.push(Term { exclude, predicate });
        }
        // Cheap metadata checks first; paths are built only for surviving files.
        terms.sort_by_key(|term| match term.predicate {
            Predicate::Size(..) => 0,
            Predicate::Extension(..) => 1,
            Predicate::Name(..) => 2,
            Predicate::Path(..) => 3,
        });
        Ok(Self { terms })
    }
    pub fn is_active(&self) -> bool {
        !self.terms.is_empty()
    }
    pub(crate) fn needs_extension(&self) -> bool {
        self.terms
            .iter()
            .any(|t| matches!(t.predicate, Predicate::Extension(..)))
    }
    pub(crate) fn matches(
        &self,
        name: &str,
        logical: u64,
        extension: &str,
        path: impl FnOnce() -> String,
    ) -> bool {
        let name = if name.is_ascii() {
            Cow::Borrowed(name)
        } else {
            Cow::Owned(name.to_lowercase())
        };
        let mut path_builder = Some(path);
        let mut path = None;
        self.terms.iter().all(|term| {
            let matched = match &term.predicate {
                Predicate::Size(comparison, bytes) => match comparison {
                    Comparison::Equal => logical == *bytes,
                    Comparison::Greater => logical > *bytes,
                    Comparison::AtLeast => logical >= *bytes,
                    Comparison::Less => logical < *bytes,
                    Comparison::AtMost => logical <= *bytes,
                },
                Predicate::Extension(extensions) => extensions.iter().any(|e| e == extension),
                Predicate::Name(needle) => contains(&name, needle),
                Predicate::Path(needle) => path
                    .get_or_insert_with(|| {
                        path_builder.take().unwrap()()
                            .replace('\\', "/")
                            .to_lowercase()
                    })
                    .contains(needle),
            };
            matched != term.exclude
        })
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    if haystack.is_ascii() {
        haystack
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
    } else {
        haystack.contains(needle)
    }
}

fn parse_bytes(input: &str) -> Result<u64, String> {
    let error = || {
        "Use a size such as size:>=1.5GB or size:<512MiB (at most 18446744073709551615 bytes)."
            .to_owned()
    };
    let end = input
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(end);
    let multiplier: u128 = match unit {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "pb" => 1_000_000_000_000_000,
        "eb" => 1_000_000_000_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1 << 40,
        "pib" => 1 << 50,
        "eib" => 1 << 60,
        _ => return Err(error()),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (number.contains('.') && fraction.is_empty())
    {
        return Err(error());
    }
    let whole = whole.parse::<u128>().map_err(|_| error())?;
    // Ignore trailing zeroes to allow exact decimals without artificial precision loss.
    let fraction = fraction.trim_end_matches('0');
    let divisor = 10u128
        .checked_pow(fraction.len() as u32)
        .ok_or_else(error)?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().map_err(|_| error())?
    };
    let scaled = fractional.checked_mul(multiplier).ok_or_else(error)?;
    if scaled % divisor != 0 {
        return Err("The size must resolve to a whole number of bytes.".into());
    }
    let bytes = whole
        .checked_mul(multiplier)
        .and_then(|n| n.checked_add(scaled / divisor))
        .ok_or_else(error)?;
    u64::try_from(bytes).map_err(|_| error())
}

#[cfg(test)]
mod tests;
