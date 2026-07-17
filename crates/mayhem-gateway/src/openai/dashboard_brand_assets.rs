// Brand assets are real repository files and are embedded into the gateway binary at compile time.
macro_rules! brand_asset {
    ($content_type:literal, $file:literal) => {
        Some((
            $content_type,
            include_bytes!(concat!("../../assets/brand-logos/", $file)).as_slice(),
        ))
    };
}

pub(super) fn dashboard_brand_asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    match name {
        "ace-step.webp" => brand_asset!("image/webp", "ace-step.webp"),
        "baai.webp" => brand_asset!("image/webp", "baai.webp"),
        "black-forest-labs.webp" => brand_asset!("image/webp", "black-forest-labs.webp"),
        "deepreinforce.webp" => brand_asset!("image/webp", "deepreinforce.webp"),
        "empero-ai.webp" => brand_asset!("image/webp", "empero-ai.webp"),
        "hauhau.svg" => brand_asset!("image/svg+xml", "hauhau.svg"),
        "hexgrad.webp" => brand_asset!("image/webp", "hexgrad.webp"),
        "huihui-ai.webp" => brand_asset!("image/webp", "huihui-ai.webp"),
        "jina-ai.webp" => brand_asset!("image/webp", "jina-ai.webp"),
        "lightricks.webp" => brand_asset!("image/webp", "lightricks.webp"),
        "lodestones.webp" => brand_asset!("image/webp", "lodestones.webp"),
        "microsoft.webp" => brand_asset!("image/webp", "microsoft.webp"),
        "nomic-ai.webp" => brand_asset!("image/webp", "nomic-ai.webp"),
        "openai-symbol.svg" => brand_asset!("image/svg+xml", "openai-symbol.svg"),
        "resemble-ai.webp" => brand_asset!("image/webp", "resemble-ai.webp"),
        "stability-ai.webp" => brand_asset!("image/webp", "stability-ai.webp"),
        "tencent.webp" => brand_asset!("image/webp", "tencent.webp"),
        "tongyi-mai.webp" => brand_asset!("image/webp", "tongyi-mai.webp"),
        "wepiqx.webp" => brand_asset!("image/webp", "wepiqx.webp"),
        "deepmind.svg" => brand_asset!("image/svg+xml", "deepmind.svg"),
        "deepseek.svg" => brand_asset!("image/svg+xml", "deepseek.svg"),
        "google.svg" => brand_asset!("image/svg+xml", "google.svg"),
        "huggingface.svg" => brand_asset!("image/svg+xml", "huggingface.svg"),
        "meta-ai.svg" => brand_asset!("image/svg+xml", "meta-ai.svg"),
        "minimax.svg" => brand_asset!("image/svg+xml", "minimax.svg"),
        "mistral.svg" => brand_asset!("image/svg+xml", "mistral.svg"),
        "moonshot-ai.svg" => brand_asset!("image/svg+xml", "moonshot-ai.svg"),
        "nvidia.svg" => brand_asset!("image/svg+xml", "nvidia.svg"),
        "qwen.svg" => brand_asset!("image/svg+xml", "qwen.svg"),
        "z-ai.svg" => brand_asset!("image/svg+xml", "z-ai.svg"),
        _ => None,
    }
}
