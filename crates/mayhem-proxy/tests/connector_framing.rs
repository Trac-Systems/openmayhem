use mayhem_proxy::connector::{
    failure::{openai_stream_error, Code, Execution, Stage},
    framing::{Decoder, Frame},
    http::WireFormat,
};
use serde_json::json;

fn frame_value(frame: Frame) -> serde_json::Value {
    match frame {
        Frame::Sse { event, data, id } => json!({"event":event,"data":data,"id":id}),
        Frame::Ndjson(v) => v,
    }
}

#[test]
fn sse_utf8_line_endings_ids_and_multiline_events_survive_every_chunk_split() {
    let input="\u{feff}: keepalive\r\nid: fixture-1\r\nevent: delta\r\ndata: café\r\ndata: 🦉\r\n\r\nretry: 1\rdata: next\r\revent: ignored\n\nid: bad\0id\ndata:\n\n: trailing comment";
    let expected = vec![
        json!({"event":"delta","data":"café\n🦉","id":"fixture-1"}),
        json!({"event":"message","data":"next","id":"fixture-1"}),
        json!({"event":"message","data":"","id":"fixture-1"}),
    ];
    for size in 1..=input.len() {
        let mut decoder = Decoder::new(WireFormat::Sse, 1024).unwrap();
        let mut frames = vec![];
        for chunk in input.as_bytes().chunks(size) {
            decoder
                .push(chunk, |frame| {
                    frames.push(frame_value(frame));
                    Ok(())
                })
                .unwrap();
        }
        decoder
            .finish(|frame| {
                frames.push(frame_value(frame));
                Ok(())
            })
            .unwrap();
        assert_eq!(frames, expected, "chunk size {size}");
    }
}

#[test]
fn ndjson_utf8_and_final_record_without_newline_are_supported() {
    let input = "\u{feff}{\"word\":\"café\"}\r\n\n{\"choice\":\"go\"}";
    let mut decoder = Decoder::new(WireFormat::Ndjson, 1024).unwrap();
    let mut frames = vec![];
    for chunk in input.as_bytes().chunks(1) {
        decoder
            .push(chunk, |frame| {
                frames.push(frame_value(frame));
                Ok(())
            })
            .unwrap();
    }
    decoder
        .finish(|frame| {
            frames.push(frame_value(frame));
            Ok(())
        })
        .unwrap();
    assert_eq!(frames, vec![json!({"word":"café"}), json!({"choice":"go"})]);
    decoder
        .finish(|_| panic!("duplicate finish event"))
        .unwrap();
}

#[test]
fn partial_invalid_or_oversized_stream_frames_do_not_finish_successfully() {
    for (format, input) in [
        (WireFormat::Sse, b"data: partial".as_slice()),
        (WireFormat::Sse, b"data: partial\n"),
        (WireFormat::Ndjson, b"{\"partial\":"),
        (WireFormat::Sse, b"data: \xff"),
    ] {
        let mut decoder = Decoder::new(format, 1024).unwrap();
        decoder
            .push(input, |_| panic!("partial frame emitted"))
            .unwrap();
        assert_eq!(
            decoder
                .finish(|_| panic!("partial frame emitted"))
                .unwrap_err()
                .code,
            Code::UpstreamProtocol
        );
        assert!(decoder.finish(|_| Ok(())).is_err());
        assert!(decoder.push(b"\n\n", |_| Ok(())).is_err());
    }
    for input in [
        b"data: 12345678901234567890\n\n".as_slice(),
        b"data: 12345\ndata: 12345\ndata: 12345\n\n",
        b"id: 1234567890\nevent: 1234567890\ndata: abc\n\n",
    ] {
        let mut decoder = Decoder::new(WireFormat::Sse, 16).unwrap();
        assert_eq!(
            decoder
                .push(input, |_| panic!("oversized frame emitted"))
                .unwrap_err()
                .code,
            Code::ResponseTooLarge
        );
        assert!(decoder.finish(|_| Ok(())).is_err());
    }
}

#[test]
fn known_stream_errors_are_classified_while_refusals_and_text_are_not() {
    for payload in [
        json!({"error":{"code":"rate_limit_exceeded","message":"private-fixture"}}),
        json!({"type":"error","code":"rate_limit_exceeded","message":"private-fixture"}),
    ] {
        let error = openai_stream_error(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert_eq!(
            (error.code, error.stage, error.execution),
            (
                Code::UpstreamRateLimited,
                Stage::ResponseBody,
                Execution::Unknown
            )
        );
        assert!(!format!("{error:?}").contains("private-fixture"));
        assert_eq!(error.upstream_status, None);
    }
    for payload in [
        json!({"choices":[{"delta":{"content":"Error: not supported"}}]}),
        json!({"choices":[{"delta":{"refusal":"I cannot help"}}]}),
        json!({"type":"response.refusal.delta","delta":"No"}),
    ] {
        assert!(openai_stream_error(&serde_json::to_vec(&payload).unwrap()).is_none());
    }
}

#[test]
fn long_event_sequence_is_emitted_incrementally_without_accumulating_history() {
    let mut decoder = Decoder::new(WireFormat::Sse, 1024).unwrap();
    let mut count = 0;
    for _ in 0..10000 {
        decoder
            .push(b"data: x\n\n", |_| {
                count += 1;
                Ok(())
            })
            .unwrap();
    }
    decoder
        .finish(|_| panic!("unexpected buffered frame"))
        .unwrap();
    assert_eq!(count, 10000);
}
