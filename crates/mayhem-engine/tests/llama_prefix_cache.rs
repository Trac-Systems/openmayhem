#![cfg(feature = "llama-cpp")]
use mayhem_engine::{
    CancellationToken, EngineBackend, GenerateRequest, LlamaCppBackend, LoadConfig,
};

#[test]
#[ignore = "requires an explicit local native model; run on the owned provider before rollout"]
fn prefix_reuse_matches_cold_generation_and_discards_changed_tail(
) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("MAYHEM_PREFIX_CACHE_TEST_MODEL")?;
    let mut config = LoadConfig::gguf(path);
    config.ctx_size = 8192;
    config.batch_size = 256;
    config.ubatch_size = 256;
    config.gpu_layers = Some(999);
    config.threads = Some(4);
    let mut backend = LlamaCppBackend::new()?;
    backend.load(config.clone())?;
    backend.set_prefix_cache_limit(2 * 1024 * 1024 * 1024);
    let prefix = "The ledger contains a green notebook and a blue pencil. ".repeat(180);
    let prompt = format!("{prefix}\nQuestion: What color is the notebook?\nAnswer:");
    let generate = |backend: &mut LlamaCppBackend, prompt: &str| {
        let mut request = GenerateRequest::new(prompt).with_max_new_tokens(20);
        request.temperature = Some(0.0);
        request.seed = Some(41);
        backend.generate(request, &mut |_| Ok(()), &CancellationToken::new())
    };
    let cold = generate(&mut backend, &prompt)?;
    assert_eq!(backend.prefix_cache_tokens().1, 0);
    let warm = generate(&mut backend, &prompt)?;
    let (total, cached) = backend.prefix_cache_tokens();
    assert!(
        cached > 1000 && cached < total && cached % config.batch_size as usize == 0,
        "cache not reused: {total}/{cached}"
    );
    assert_eq!(cold.text, warm.text, "warm cache changed greedy output");
    assert_eq!(cold.usage.prompt_tokens, warm.usage.prompt_tokens);
    let appended = format!("{prompt}\nReply with only the color.");
    let warm_appended = generate(&mut backend, &appended)?;
    assert!(
        backend.prefix_cache_tokens().1 > 1000,
        "appended turn missed the shared prefix"
    );
    backend.set_prefix_cache_limit(0);
    let cold_appended = generate(&mut backend, &appended)?;
    assert_eq!(
        warm_appended.text, cold_appended.text,
        "appended prefix changed greedy output"
    );
    backend.set_prefix_cache_limit(2 * 1024 * 1024 * 1024);
    generate(&mut backend, &prompt)?;
    let changed = format!("{prefix}\nQuestion: What color is the pencil?\nAnswer:");
    let warm_changed = generate(&mut backend, &changed)?;
    // Recurrent/SWA models cannot necessarily roll back inside a snapshot.
    // They must fall back to an equivalent cold result, without stale suffixes.
    let changed_cached = backend.prefix_cache_tokens().1;
    assert!(changed_cached == 0 || changed_cached > 1000);
    backend.set_prefix_cache_limit(0);
    let cold_changed = generate(&mut backend, &changed)?;
    assert_eq!(
        warm_changed.text, cold_changed.text,
        "stale suffix influenced output"
    );
    backend.set_prefix_cache_limit(2 * 1024 * 1024 * 1024);
    let chat_start = format!("{prefix}\nUser: What color is the notebook?\nAssistant:\n<think>");
    let chat_second = format!("{prefix}\nUser: What color is the notebook?\nAssistant: green\nUser: Reply with the color again.\nAssistant:\n<think>");
    let chat_third = format!("{prefix}\nUser: What color is the notebook?\nAssistant: green\nUser: Reply with only the color.\nAssistant:\n<think>");
    generate(&mut backend, &chat_start)?;
    generate(&mut backend, &chat_second)?;
    let warm_chat = generate(&mut backend, &chat_third)?;
    assert!(
        backend.prefix_cache_tokens().1 > 1000,
        "changing chat suffixes must learn a reusable recurrent/SWA checkpoint"
    );
    backend.set_prefix_cache_limit(0);
    let cold_chat = generate(&mut backend, &chat_third)?;
    assert_eq!(
        warm_chat.text, cold_chat.text,
        "learned prefix changed greedy output"
    );
    backend.set_prefix_cache_limit(2 * 1024 * 1024 * 1024);
    generate(&mut backend, &prompt)?;
    generate(&mut backend, "Write the number seven in words. Answer:")?;
    assert!(backend.prefix_cache_tokens().1 < 4);
    backend.load(config)?;
    assert_eq!(backend.prefix_cache_tokens(), (0, 0));
    generate(&mut backend, &prompt)?;
    assert_eq!(backend.prefix_cache_tokens().1, 0);
    backend.set_prefix_cache_limit(1);
    generate(&mut backend, &prompt)?;
    generate(&mut backend, &prompt)?;
    assert_eq!(
        backend.prefix_cache_tokens().1,
        0,
        "byte limit must bound retained state"
    );
    Ok(())
}
