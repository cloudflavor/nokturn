use std::str::FromStr;

#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
    Patch,
    Options,
    Head,
    Trace,
}

impl FromStr for HttpMethod {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "get" | "GET" => Ok(Self::Get),
            "put" | "PUT" => Ok(Self::Put),
            "post" | "POST" => Ok(Self::Post),
            "delete" | "DELETE" => Ok(Self::Delete),
            "patch" | "PATCH" => Ok(Self::Patch),
            "options" | "OPTIONS" => Ok(Self::Options),
            "head" | "HEAD" => Ok(Self::Head),
            "trace" | "TRACE" => Ok(Self::Trace),
            _ => Err(()),
        }
    }
}

pub fn decode_pointer_segment(segment: &str) -> String {
    let mut result = String::with_capacity(segment.len());
    let mut chars = segment.chars();

    while let Some(ch) = chars.next() {
        if ch == '~' {
            match chars.next() {
                Some('0') => result.push('~'),
                Some('1') => result.push('/'),
                Some(other) => {
                    result.push('~');
                    result.push(other);
                }
                None => result.push('~'),
            }
        } else {
            result.push(ch);
        }
    }

    result
}
