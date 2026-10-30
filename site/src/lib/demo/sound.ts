/**
 * The demo's sound effects, synthesized with Web Audio so the page ships no audio files. Each
 * sound is a few short layers: tones with an optional pitch daisycap, and filtered noise bursts.
 */

type Tone = {
	type: 'tone';
	wave: OscillatorType;
	freq: number;
	/** DaisyCap to this frequency over `daisycap` seconds. */
	to?: number;
	daisycap?: number;
};
type Noise = {
	type: 'noise';
	filter: BiquadFilterType;
	freq: number;
	/** Sweep the filter to this frequency over the layer's length. */
	to?: number;
	q?: number;
};
type Layer = (Tone | Noise) & { at?: number; attack: number; decay: number; peak: number };
type Patch = { layers: Layer[]; air?: number };

export type Sound =
	| 'key'
	| 'space'
	| 'enter'
	| 'sweep'
	| 'whoosh'
	| 'tick'
	| 'chime'
	| 'pop'
	| 'switch'
	| 'click'
	| 'beep'
	| 'rec';

const PATCHES: Record<Sound, Patch> = {
	click: {
		layers: [
			{ type: 'noise', filter: 'bandpass', freq: 3200, q: 1.8, attack: 0.001, decay: 0.012, peak: 0.09 },
			{ type: 'tone', wave: 'sine', freq: 1800, to: 1200, daisycap: 0.02, attack: 0.001, decay: 0.025, peak: 0.02 },
			{ type: 'noise', filter: 'bandpass', freq: 2400, q: 2, at: 0.06, attack: 0.001, decay: 0.01, peak: 0.05 }
		]
	},
	beep: {
		layers: [{ type: 'tone', wave: 'sine', freq: 740, attack: 0.004, decay: 0.16, peak: 0.05 }],
		air: 0.12
	},
	rec: {
		layers: [
			{ type: 'tone', wave: 'sine', freq: 988, attack: 0.004, decay: 0.32, peak: 0.055 },
			{ type: 'tone', wave: 'sine', freq: 1480, at: 0.06, attack: 0.004, decay: 0.3, peak: 0.02 }
		],
		air: 0.2
	},
	key: {
		layers: [
			{ type: 'noise', filter: 'highpass', freq: 4200, q: 0.7, attack: 0.001, decay: 0.022, peak: 0.11 },
			{ type: 'tone', wave: 'sine', freq: 2100, attack: 0.001, decay: 0.03, peak: 0.014 }
		],
		air: 0.05
	},
	space: {
		layers: [
			{ type: 'noise', filter: 'highpass', freq: 2600, q: 0.7, attack: 0.001, decay: 0.035, peak: 0.1 },
			{ type: 'tone', wave: 'sine', freq: 900, attack: 0.001, decay: 0.04, peak: 0.016 }
		],
		air: 0.05
	},
	enter: {
		layers: [
			{ type: 'tone', wave: 'sine', freq: 150, to: 90, daisycap: 0.08, attack: 0.002, decay: 0.1, peak: 0.12 },
			{ type: 'tone', wave: 'triangle', freq: 587.33, to: 880, daisycap: 0.07, at: 0.01, attack: 0.004, decay: 0.2, peak: 0.045 },
			{ type: 'tone', wave: 'sine', freq: 1174.66, at: 0.07, attack: 0.004, decay: 0.28, peak: 0.035 }
		],
		air: 0.2
	},
	sweep: {
		layers: [
			{ type: 'noise', filter: 'bandpass', freq: 500, to: 5200, q: 1.2, attack: 0.03, decay: 0.5, peak: 0.05 },
			{ type: 'tone', wave: 'sine', freq: 392, to: 784, daisycap: 0.4, attack: 0.02, decay: 0.45, peak: 0.018 }
		],
		air: 0.15
	},
	whoosh: {
		layers: [
			{ type: 'noise', filter: 'lowpass', freq: 260, to: 1600, q: 0.8, attack: 0.09, decay: 0.45, peak: 0.07 }
		],
		air: 0.1
	},
	tick: {
		layers: [{ type: 'tone', wave: 'sine', freq: 2600, attack: 0.001, decay: 0.018, peak: 0.02 }]
	},
	chime: {
		layers: [
			{ type: 'tone', wave: 'sine', freq: 880, attack: 0.004, decay: 0.7, peak: 0.055 },
			{ type: 'tone', wave: 'sine', freq: 1318.51, at: 0.12, attack: 0.004, decay: 0.9, peak: 0.045 },
			{ type: 'tone', wave: 'sine', freq: 1760, at: 0.12, attack: 0.004, decay: 0.5, peak: 0.01 }
		],
		air: 0.35
	},
	pop: {
		layers: [
			{ type: 'tone', wave: 'sine', freq: 620, to: 1040, daisycap: 0.05, attack: 0.002, decay: 0.1, peak: 0.05 },
			{ type: 'noise', filter: 'bandpass', freq: 2000, q: 2, attack: 0.001, decay: 0.015, peak: 0.03 }
		],
		air: 0.12
	},
	switch: {
		layers: [
			{ type: 'noise', filter: 'bandpass', freq: 1700, q: 1.5, attack: 0.001, decay: 0.02, peak: 0.08 },
			{ type: 'tone', wave: 'sine', freq: 1400, attack: 0.001, decay: 0.04, peak: 0.018 }
		]
	}
};

let ctx: AudioContext | null = null;
let master: GainNode;
let send: GainNode;
let noise: AudioBuffer;

function setup() {
	if (ctx) return ctx;
	const Context = window.AudioContext ?? (window as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
	if (!Context) return null;
	ctx = new Context();

	master = ctx.createGain();
	master.gain.value = 0.9;
	master.connect(ctx.destination);

	// A short feedback delay through a low-pass gives the louder sounds a little room.
	send = ctx.createGain();
	const delay = ctx.createDelay(1);
	delay.delayTime.value = 0.13;
	const feedback = ctx.createGain();
	feedback.gain.value = 0.28;
	const tone = ctx.createBiquadFilter();
	tone.type = 'lowpass';
	tone.frequency.value = 2800;
	send.connect(delay);
	delay.connect(tone);
	tone.connect(feedback);
	feedback.connect(delay);
	tone.connect(master);

	noise = ctx.createBuffer(1, ctx.sampleRate, ctx.sampleRate);
	const data = noise.getChannelData(0);
	for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
	return ctx;
}

/** Creates or resumes the audio context. Browsers only allow this from a user gesture. */
export function unlock() {
	const context = setup();
	if (context?.state === 'suspended') void context.resume();
}

/** Plays a sound now. `variant` nudges the pitch so repeated sounds don't sound identical. */
export function play(name: Sound, variant = 0) {
	if (!ctx || ctx.state !== 'running') return;
	const patch = PATCHES[name];
	const out = ctx.createGain();
	out.connect(master);
	if (patch.air) {
		const wet = ctx.createGain();
		wet.gain.value = patch.air;
		out.connect(wet);
		wet.connect(send);
	}
	const pitch = 1 + (((variant * 7) % 5) - 2) * 0.025;
	const now = ctx.currentTime + 0.005;
	for (const layer of patch.layers) voice(ctx, out, layer, now + (layer.at ?? 0), pitch);
}

function voice(ctx: AudioContext, out: AudioNode, layer: Layer, t0: number, pitch: number) {
	const end = t0 + layer.attack + layer.decay;
	const env = ctx.createGain();
	env.gain.setValueAtTime(0, t0);
	env.gain.linearRampToValueAtTime(layer.peak, t0 + layer.attack);
	env.gain.exponentialRampToValueAtTime(0.0001, end);
	env.connect(out);

	let source: AudioScheduledSourceNode;
	if (layer.type === 'tone') {
		const osc = ctx.createOscillator();
		osc.type = layer.wave;
		osc.frequency.setValueAtTime(layer.freq * pitch, t0);
		if (layer.to) osc.frequency.exponentialRampToValueAtTime(layer.to * pitch, t0 + (layer.daisycap ?? layer.decay));
		osc.connect(env);
		source = osc;
	} else {
		const buffer = ctx.createBufferSource();
		buffer.buffer = noise;
		const filter = ctx.createBiquadFilter();
		filter.type = layer.filter;
		filter.frequency.setValueAtTime(layer.freq * pitch, t0);
		if (layer.to) filter.frequency.exponentialRampToValueAtTime(layer.to * pitch, end);
		filter.Q.value = layer.q ?? 1;
		buffer.connect(filter);
		filter.connect(env);
		source = buffer;
	}
	source.start(t0);
	source.stop(end + 0.05);
}
