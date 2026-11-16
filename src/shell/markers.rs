//! Detection of the invisible markers that the injected shell scripts emit.

/// A marker is `ESC ] 7777 ; gitoui ; <nonce> ; <payload> BEL`.
pub(crate) const MARKER_PREFIX: &[u8] = b"\x1b]7777;gitoui;";

/// A candidate that grows past this many bytes without a BEL is plain output.
const MAX_CANDIDATE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Marker {
    /// The shell shows a prompt; this is its current directory.
    Cwd(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    Output(Vec<u8>),
    Marker(Marker),
}

pub(crate) struct MarkerScanner {
    nonce_prefix: Vec<u8>,
    pending: Vec<u8>,
}

impl MarkerScanner {
    pub(crate) fn new(nonce: &str) -> Self {
        Self {
            nonce_prefix: format!("{nonce};").into_bytes(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<Segment> {
        let mut work = std::mem::take(&mut self.pending);
        work.extend_from_slice(bytes);
        let mut segments = Vec::new();
        let mut out: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < work.len() {
            let Some(offset) = work[i..].iter().position(|&b| b == 0x1b) else {
                out.extend_from_slice(&work[i..]);
                i = work.len();
                break;
            };
            let p = i + offset;
            out.extend_from_slice(&work[i..p]);
            let rest = &work[p..];
            if rest.len() < MARKER_PREFIX.len() {
                if MARKER_PREFIX.starts_with(rest) {
                    self.pending = rest.to_vec();
                    i = work.len();
                    break;
                }
                out.push(0x1b);
                i = p + 1;
                continue;
            }
            if !rest.starts_with(MARKER_PREFIX) {
                out.push(0x1b);
                i = p + 1;
                continue;
            }
            let Some(bel) = rest.iter().position(|&b| b == 0x07) else {
                if rest.len() > MAX_CANDIDATE {
                    out.push(0x1b);
                    i = p + 1;
                } else {
                    self.pending = rest.to_vec();
                    i = work.len();
                    break;
                }
                continue;
            };
            let body = &rest[MARKER_PREFIX.len()..bel];
            let marker = body
                .strip_prefix(self.nonce_prefix.as_slice())
                .and_then(parse_payload);
            match marker {
                Some(marker) => {
                    if !out.is_empty() {
                        segments.push(Segment::Output(std::mem::take(&mut out)));
                    }
                    segments.push(Segment::Marker(marker));
                }
                None => out.extend_from_slice(&rest[..=bel]),
            }
            i = p + bel + 1;
        }
        let _ = i;
        if !out.is_empty() {
            segments.push(Segment::Output(out));
        }
        segments
    }
}

fn parse_payload(payload: &[u8]) -> Option<Marker> {
    let text = String::from_utf8_lossy(payload);
    text.strip_prefix("C;")
        .map(|dir| Marker::Cwd(dir.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(payload: &str) -> Vec<u8> {
        format!("\x1b]7777;gitoui;n0nc3;{payload}\x07").into_bytes()
    }

    #[test]
    fn passes_plain_output_through() {
        assert_eq!(
            MarkerScanner::new("n0nc3").feed(b"hello"),
            vec![Segment::Output(b"hello".to_vec())]
        );
    }

    fn sample() -> Vec<u8> {
        [m("C;/home/u/repo"), b"out\r\n".to_vec(), m("C;/tmp")].concat()
    }

    fn expected_sample() -> Vec<Segment> {
        vec![
            Segment::Marker(Marker::Cwd("/home/u/repo".into())),
            Segment::Output(b"out\r\n".to_vec()),
            Segment::Marker(Marker::Cwd("/tmp".into())),
        ]
    }

    #[test]
    fn parses_the_directory_marker() {
        assert_eq!(
            MarkerScanner::new("n0nc3").feed(&sample()),
            expected_sample()
        );
    }

    #[test]
    fn reassembles_markers_split_at_any_byte() {
        let input = sample();
        for i in 0..=input.len() {
            let mut scanner = MarkerScanner::new("n0nc3");
            let mut segments = scanner.feed(&input[..i]);
            segments.extend(scanner.feed(&input[i..]));
            let markers = segments
                .iter()
                .filter(|s| matches!(s, Segment::Marker(_)))
                .count();
            let output: Vec<u8> = segments
                .iter()
                .filter_map(|s| match s {
                    Segment::Output(o) => Some(o.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            assert_eq!(markers, 2, "split at {i}");
            assert_eq!(output, b"out\r\n".to_vec(), "split at {i}");
        }
    }

    #[test]
    fn foreign_nonce_and_other_osc_pass_through() {
        let input = [
            b"\x1b]7;file://h/x\x07".to_vec(),
            "\x1b]7777;gitoui;other;S\x07".as_bytes().to_vec(),
        ]
        .concat();
        assert_eq!(
            MarkerScanner::new("n0nc3").feed(&input),
            vec![Segment::Output(input.clone())]
        );
    }

    #[test]
    fn unterminated_candidate_is_released() {
        let mut input = MARKER_PREFIX.to_vec();
        input.extend_from_slice(b"n0nc3;");
        input.extend(std::iter::repeat_n(b'a', 5000));
        let total: usize = MarkerScanner::new("n0nc3")
            .feed(&input)
            .iter()
            .map(|s| match s {
                Segment::Output(o) => o.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(total, input.len());
    }

    #[test]
    fn unknown_payloads_are_dropped_from_the_output_stream_untouched() {
        let unknown = m("Z;whatever");
        assert_eq!(
            MarkerScanner::new("n0nc3").feed(&unknown),
            vec![Segment::Output(unknown.clone())]
        );
    }
}
