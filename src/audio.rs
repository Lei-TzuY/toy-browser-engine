//! Web Audio API Synthesizer Subsystem
//!
//! Provides the core graph representation, node types (OscillatorNode, GainNode,
//! AudioDestinationNode), parameter automation, and PCM sample rendering.

use std::f32::consts::PI;

/// Oscillator waveform types supported by the Web Audio API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscillatorType {
    Sine,
    Square,
    Sawtooth,
    Triangle,
}

impl OscillatorType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sine" => Some(OscillatorType::Sine),
            "square" => Some(OscillatorType::Square),
            "sawtooth" => Some(OscillatorType::Sawtooth),
            "triangle" => Some(OscillatorType::Triangle),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            OscillatorType::Sine => "sine",
            OscillatorType::Square => "square",
            OscillatorType::Sawtooth => "sawtooth",
            OscillatorType::Triangle => "triangle",
        }
    }
}

/// Represents an automatable audio parameter (e.g. gain, frequency).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioParam {
    pub value: f32,
    pub default_value: f32,
    pub min_value: f32,
    pub max_value: f32,
}

impl AudioParam {
    pub fn new(default_value: f32, min_val: f32, max_val: f32) -> Self {
        Self {
            value: default_value,
            default_value,
            min_value: min_val,
            max_value: max_val,
        }
    }

    pub fn set_value(&mut self, val: f32) {
        self.value = val.clamp(self.min_value, self.max_value);
    }
}

/// The specific data and state for each kind of audio node.
#[derive(Debug, Clone)]
pub enum AudioNodeKind {
    Destination,
    Gain {
        gain: AudioParam,
    },
    Oscillator {
        osc_type: OscillatorType,
        frequency: AudioParam,
        started: bool,
        stopped: bool,
    },
}

/// An individual node in the audio processing graph.
#[derive(Debug, Clone)]
pub struct AudioNode {
    pub id: usize,
    pub kind: AudioNodeKind,
    pub outputs: Vec<usize>,
}

/// The main AudioContext managing node creation and sample generation.
#[derive(Debug, Clone)]
pub struct AudioContext {
    pub sample_rate: f32,
    pub state: String,
    pub destination_id: usize,
    pub nodes: Vec<AudioNode>,
    next_id: usize,
}

impl AudioContext {
    pub fn new() -> Self {
        Self::with_sample_rate(44100.0)
    }

    pub fn with_sample_rate(sample_rate: f32) -> Self {
        let dest = AudioNode {
            id: 0,
            kind: AudioNodeKind::Destination,
            outputs: Vec::new(),
        };
        Self {
            sample_rate,
            state: "running".to_string(),
            destination_id: 0,
            nodes: vec![dest],
            next_id: 1,
        }
    }

    pub fn create_oscillator(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(AudioNode {
            id,
            kind: AudioNodeKind::Oscillator {
                osc_type: OscillatorType::Sine,
                frequency: AudioParam::new(440.0, 0.0, self.sample_rate / 2.0),
                started: false,
                stopped: false,
            },
            outputs: Vec::new(),
        });
        id
    }

    pub fn create_gain(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(AudioNode {
            id,
            kind: AudioNodeKind::Gain {
                gain: AudioParam::new(1.0, -3.4028235e38, 3.4028235e38),
            },
            outputs: Vec::new(),
        });
        id
    }

    pub fn get_node(&self, id: usize) -> Option<&AudioNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn get_node_mut(&mut self, id: usize) -> Option<&mut AudioNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn connect(&mut self, source_id: usize, dest_id: usize) -> bool {
        if self.get_node(dest_id).is_none() {
            return false;
        }
        if let Some(src) = self.get_node_mut(source_id) {
            if !src.outputs.contains(&dest_id) {
                src.outputs.push(dest_id);
            }
            true
        } else {
            false
        }
    }

    pub fn disconnect(&mut self, source_id: usize) -> bool {
        if let Some(src) = self.get_node_mut(source_id) {
            src.outputs.clear();
            true
        } else {
            false
        }
    }

    /// Synthesizes PCM audio samples for a given duration in seconds.
    pub fn render_pcm_buffer(&self, duration_secs: f32) -> Vec<f32> {
        let num_samples = (duration_secs * self.sample_rate).round() as usize;
        let mut buffer = vec![0.0f32; num_samples];

        // Find all active oscillator nodes
        for node in &self.nodes {
            if let AudioNodeKind::Oscillator {
                osc_type,
                frequency,
                started,
                stopped,
            } = &node.kind
            {
                if !*started || *stopped {
                    continue;
                }

                // Check if this oscillator reaches destination through gain nodes
                let total_gain = self.trace_gain_to_destination(node.id, 1.0);
                if total_gain == 0.0 {
                    continue;
                }

                let freq = frequency.value;
                for (n, sample) in buffer.iter_mut().enumerate() {
                    let t = n as f32 / self.sample_rate;
                    let wave = match osc_type {
                        OscillatorType::Sine => (2.0 * PI * freq * t).sin(),
                        OscillatorType::Square => {
                            if (2.0 * PI * freq * t).sin() >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        OscillatorType::Sawtooth => {
                            let phase = freq * t;
                            2.0 * (phase - (phase + 0.5).floor())
                        }
                        OscillatorType::Triangle => {
                            let phase = freq * t;
                            2.0 * (2.0 * (phase - (phase + 0.5).floor())).abs() - 1.0
                        }
                    };
                    *sample += wave * total_gain;
                }
            }
        }

        // Clamp final PCM buffer to [-1.0, 1.0]
        for s in &mut buffer {
            *s = s.clamp(-1.0, 1.0);
        }

        buffer
    }

    fn trace_gain_to_destination(&self, current_id: usize, current_gain: f32) -> f32 {
        if current_id == self.destination_id {
            return current_gain;
        }

        let Some(node) = self.get_node(current_id) else {
            return 0.0;
        };

        let mut total = 0.0f32;
        for &next_id in &node.outputs {
            let next_node = self.get_node(next_id);
            let next_gain = if let Some(n) = next_node {
                match &n.kind {
                    AudioNodeKind::Gain { gain } => current_gain * gain.value,
                    _ => current_gain,
                }
            } else {
                current_gain
            };
            total += self.trace_gain_to_destination(next_id, next_gain);
        }

        total
    }
}
