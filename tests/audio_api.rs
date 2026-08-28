use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_audio_context_nodes_and_params() {
    let doc = run_js(r#"
        let ctx = new AudioContext();
        console.log("sampleRate:" + ctx.sampleRate);
        console.log("state:" + ctx.state);

        let osc = ctx.createOscillator();
        let gain = ctx.createGain();

        osc.type = "sawtooth";
        osc.frequency.value = 880;
        gain.gain.value = 0.5;

        console.log("osc_type:" + osc.type);
        console.log("osc_freq:" + osc.frequency.value);
        console.log("gain_val:" + gain.gain.value);

        osc.connect(gain);
        gain.connect(ctx.destination);

        osc.start();
        console.log("started:true");
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "sampleRate:44100");
    assert_eq!(logs[1], "state:running");
    assert_eq!(logs[2], "osc_type:sawtooth");
    assert_eq!(logs[3], "osc_freq:880");
    assert_eq!(logs[4], "gain_val:0.5");
    assert_eq!(logs[5], "started:true");
}

#[test]
fn test_audio_pcm_synthesis_buffer() {
    use browser_engine::audio::{AudioContext, AudioNodeKind, OscillatorType};

    let mut ctx = AudioContext::new();
    let osc_id = ctx.create_oscillator();
    let gain_id = ctx.create_gain();

    if let Some(node) = ctx.get_node_mut(osc_id) {
        if let AudioNodeKind::Oscillator {
            ref mut osc_type,
            ref mut frequency,
            ref mut started,
            ..
        } = node.kind
        {
            *osc_type = OscillatorType::Sine;
            frequency.set_value(440.0);
            *started = true;
        }
    }

    if let Some(node) = ctx.get_node_mut(gain_id) {
        if let AudioNodeKind::Gain { ref mut gain } = node.kind {
            gain.set_value(0.8);
        }
    }

    ctx.connect(osc_id, gain_id);
    ctx.connect(gain_id, ctx.destination_id);

    // Render 0.1 seconds of audio
    let pcm = ctx.render_pcm_buffer(0.1);
    assert_eq!(pcm.len(), 4410); // 44100 * 0.1

    // Ensure non-zero generated PCM wave and bounded by gain 0.8
    let max_sample = pcm.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    assert!(max_sample > 0.7);
    assert!(max_sample <= 0.8001);
}
