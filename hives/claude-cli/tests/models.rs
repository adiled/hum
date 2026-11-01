#[test]
fn the_models_advertised_are_the_ones_named_or_the_defaults() {
    let defaults: Vec<String> = claude_cli::DEFAULT_MODELS.iter().map(|m| m.to_string()).collect();

    unsafe { std::env::remove_var("CLAUDE_MODELS") };
    assert_eq!(claude_cli::advertised_models(), defaults);

    for unnamed in ["", " ", ",", " , "] {
        unsafe { std::env::set_var("CLAUDE_MODELS", unnamed) };
        assert_eq!(claude_cli::advertised_models(), defaults, "{unnamed:?} names no model");
    }

    unsafe { std::env::set_var("CLAUDE_MODELS", " claude-opus-5-5, ,claude-haiku-5-5 ") };
    assert_eq!(claude_cli::advertised_models(), vec!["claude-opus-5-5", "claude-haiku-5-5"]);
}
