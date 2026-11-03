//! Bounded metadata for rejected text output. Never stores prompts or output text.

#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputShape {
    pub stage: &'static str,
    pub input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub output_limit: Option<u64>,
    pub finish: Finish,
    pub text_bytes: usize,
    pub tools: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Finish {
    Stop,
    Length,
    Tools,
    Other,
}

impl Finish {
    pub fn from_label(label: &str) -> Self {
        match label {
            "stop" => Self::Stop,
            "length" => Self::Length,
            "tool_calls" => Self::Tools,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for OutputShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stage={} input_tokens={:?} output_tokens={} reasoning_tokens={} output_limit={:?} finish={:?} text_bytes={} tools={:?}",
            self.stage, self.input_tokens, self.output_tokens, self.reasoning_tokens,
            self.output_limit, self.finish, self.text_bytes, self.tools)
    }
}

impl std::error::Error for OutputShape {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_labels_never_retain_upstream_text() {
        assert_eq!(Finish::from_label("stop"), Finish::Stop);
        assert_eq!(Finish::from_label("length"), Finish::Length);
        assert_eq!(Finish::from_label("tool_calls"), Finish::Tools);
        let shape = OutputShape {
            stage: "native_validation",
            input_tokens: Some(229_000),
            output_tokens: 12,
            reasoning_tokens: 12,
            output_limit: Some(12),
            finish: Finish::from_label("PRIVATE_MODEL_OUTPUT"),
            text_bytes: 51,
            tools: None,
        };
        let rendered = shape.to_string();
        assert!(rendered.contains("output_limit=Some(12)"));
        assert!(rendered.contains("finish=Other"));
        assert!(!rendered.contains("PRIVATE_MODEL_OUTPUT"));
        assert!(rendered.len() < 256);
    }
}
