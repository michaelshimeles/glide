import { easeInOut, lerp, span } from '#lib/ease.js';
import { Recording } from '#lib/ui/recording.js';
import { button, type Point } from '#lib/ui/screen.js';
import { WALLPAPER_IDS, type WallpaperId } from '#lib/ui/wallpaper.js';
import type { Sound } from './sound.js';

/** The scene is laid out at this size and scaled to fit. */
export const STAGE = { w: 1200, h: 720 };

/** The DaisyCap window, laid out at its real size in points and scaled onto the stage. */
export const APP = { x: 80, y: 62, w: 1320, h: 792, scale: 1040 / 1320 };
/** The window being recorded, on the desktop behind DaisyCap. */
export const DESK = { x: 192, y: 70, scale: 0.85 };
/** The recording control bar (286×44 points), centred near the bottom of the display. */
export const BAR = { x: 600 - (286 * 0.9) / 2, y: 584, scale: 0.9 };

export const fromApp = (p: Point): Point => ({ x: APP.x + p.x * APP.scale, y: APP.y + p.y * APP.scale });
export const fromDesk = (p: Point): Point => ({ x: DESK.x + p.x * DESK.scale, y: DESK.y + p.y * DESK.scale });
const toDesk = (p: Point): Point => ({ x: (p.x - DESK.x) / DESK.scale, y: (p.y - DESK.y) / DESK.scale });

/** Editor regions in window points: preview above the timeline, inspector on the right. */
export const EDITOR = {
	preview: { x: 0, y: 40, w: 1000, h: 516 },
	canvas: { w: 960, h: 480 },
	timeline: { x: 12, y: 564, w: 972 },
	sheet: { x: 472, y: 132 }
};

/** Where the pointer clicks, in window points, measured from the rendered views. Slider entries are the track's left edge. */
export const TARGETS = {
	window: { x: 1134, y: 143 },
	record: { x: 38, y: 758.5 },
	play: { x: 28, y: 577 },
	cursor: { x: 1129.4, y: 60 },
	background: { x: 1056.1, y: 60 },
	smooth: { x: 1043.8, y: 280 },
	off: { x: 1212.1, y: 280 },
	swatches: { x: 1040, y: 180, step: 58 },
	padding: { x: 1210, y: 319 },
	roundness: { x: 1210, y: 372 },
	export: { x: 1278.4, y: 19.5 },
	go: { x: 803.7, y: 392 }
};

const swatch = (id: WallpaperId): Point => {
	const i = WALLPAPER_IDS.indexOf(id);
	return { x: TARGETS.swatches.x + (i % 5) * TARGETS.swatches.step, y: TARGETS.swatches.y + Math.floor(i / 5) * 40 };
};
/** A slider knob's centre for `value` out of `max`: the track is 96 points with a 15-point knob. */
const knob = (at: Point, value: number, max: number): Point => ({ x: at.x + 7.5 + (value / max) * 81, y: at.y });

/** The recording made in the first chapter, in the recorded window's points. */
const REST = { x: 640, y: 430 };
const A = button(0, 0);
const B = button(1, 1);
const STOP = toDesk({ x: BAR.x + 259 * BAR.scale, y: BAR.y + 22 * BAR.scale });
export const REC = new Recording(
	3000,
	REST,
	[
		{ start: 150, end: 800, to: A },
		{ start: 1250, end: 1900, to: B },
		{ start: 2350, end: 2850, to: STOP }
	],
	[900, 2000],
	[
		{ start: 650, end: 1500, at: A, level: 1.8 },
		{ start: 1500, end: 2500, at: B, level: 1.8 }
	]
);
/** The dashboard button under a click while it's held. */
export function pressedButton(r: number) {
	const click = REC.clicks.find((c) => r >= c.t && r < c.t + 160);
	if (!click) return null;
	return click.at === REC.clicks[0].at ? '0,0' : '1,1';
}

export const T = {
	source: 1000,
	record: 1800,
	hide: 1850,
	bar: 2050,
	count: [2200, 2800, 3400],
	rec: 4000,
	stop: 6950,
	editor: 7300,
	pops: [7950, 8100],
	play: 8450,
	playback: 8550,
	cursor: 12_700,
	off: 13_400,
	smooth: 14_900,
	background: 17_400,
	swatches: [18_050, 18_550, 19_050] as const,
	padding: [19_700, 19_750, 20_300],
	roundness: [20_850, 20_900, 21_400],
	export: 22_500,
	sheet: 22_550,
	go: 23_500,
	progress: [23_550, 24_550],
	done: 24_600
} as const;

export const DURATION = 27_000;
/** With reduced motion the scene holds here: zoomed in on the first click, caption on screen. */
export const SETTLED = 9_750;

export const CHAPTERS = [
	{ label: 'Record', t: 0 },
	{ label: 'Zoom', t: 7200 },
	{ label: 'Cursor', t: 12_000 },
	{ label: 'Style', t: 16_800 },
	{ label: 'Export', t: 21_800 }
] as const;

/** Playhead position in the recording. */
export function playhead(t: number) {
	if (t < T.off) return Math.min(REC.duration, Math.max(0, t - T.playback));
	if (t < T.smooth) return 1150 + Math.min(900, Math.max(0, t - T.off - 150));
	return 1150 + Math.min(REC.duration - 1150, Math.max(0, t - T.smooth - 150));
}

export const playing = (t: number) =>
	(t > T.playback && t < T.playback + REC.duration) ||
	(t > T.off + 150 && t < T.off + 1050) ||
	(t > T.smooth + 150 && t < T.smooth + 150 + REC.duration - 1150);

export function wallpaper(t: number): { id: WallpaperId; from?: WallpaperId; fade: number } {
	const order: WallpaperId[] = ['aurora', 'ocean', 'grape', 'lagoon'];
	const i = T.swatches.filter((s) => t >= s).length;
	if (i === 0) return { id: 'aurora', fade: 1 };
	return { id: order[i], from: order[i - 1], fade: span(t, T.swatches[i - 1], T.swatches[i - 1] + 320) };
}

export const padding = (t: number) => lerp(9, 14, easeInOut(span(t, T.padding[1], T.padding[2])));
export const roundness = (t: number) => lerp(20, 32, easeInOut(span(t, T.roundness[1], T.roundness[2])));

type DaisyCap = { start: number; end: number; to: Point; drag?: boolean };

const BEFORE: DaisyCap[] = [
	{ start: 300, end: 900, to: fromApp(TARGETS.window) },
	{ start: 1150, end: 1700, to: fromApp(TARGETS.record) },
	{ start: 2350, end: 3300, to: fromDesk(REST) }
];
const AFTER: DaisyCap[] = [
	{ start: 7500, end: 8350, to: fromApp(TARGETS.play) },
	{ start: 12_100, end: 12_600, to: fromApp(TARGETS.cursor) },
	{ start: 12_950, end: 13_300, to: fromApp(TARGETS.off) },
	{ start: 14_450, end: 14_800, to: fromApp(TARGETS.smooth) },
	{ start: 16_950, end: 17_300, to: fromApp(TARGETS.background) },
	{ start: 17_600, end: 17_950, to: fromApp(swatch('ocean')) },
	{ start: 18_200, end: 18_450, to: fromApp(swatch('grape')) },
	{ start: 18_700, end: 18_950, to: fromApp(swatch('lagoon')) },
	{ start: 19_250, end: 19_650, to: fromApp(knob(TARGETS.padding, 9, 40)) },
	{ start: T.padding[1], end: T.padding[2], to: fromApp(knob(TARGETS.padding, 14, 40)), drag: true },
	{ start: 20_400, end: 20_800, to: fromApp(knob(TARGETS.roundness, 20, 60)) },
	{ start: T.roundness[1], end: T.roundness[2], to: fromApp(knob(TARGETS.roundness, 32, 60)), drag: true },
	{ start: 21_900, end: 22_400, to: fromApp(TARGETS.export) },
	{ start: 22_950, end: 23_400, to: fromApp(TARGETS.go) },
	{ start: 25_000, end: 25_700, to: fromApp({ x: 930, y: 470 }) }
];

/** A hand-like move along a slight arc; drags stay straight so the knob tracks the pointer. */
function daisycap(a: Point, b: Point, p: number, drag = false): Point {
	if (drag) {
		const q = easeInOut(p);
		return { x: lerp(a.x, b.x, q), y: lerp(a.y, b.y, q) };
	}
	const q = p * p * p * (10 - 15 * p + 6 * p * p);
	const arc = Math.sin(Math.PI * q) * 0.08;
	return { x: lerp(a.x, b.x, q) - (b.y - a.y) * arc, y: lerp(a.y, b.y, q) + (b.x - a.x) * arc };
}

/** The system pointer, in stage coordinates. While recording it is the recording's raw path. */
export function pointer(t: number): Point {
	if (t >= T.rec && t <= T.stop) return fromDesk(REC.raw(t - T.rec));
	const glides = t < T.rec ? BEFORE : AFTER;
	let from = t < T.rec ? { x: 760, y: 470 } : fromDesk(REC.raw(T.stop - T.rec));
	for (const g of glides) {
		if (t < g.start) return from;
		if (t < g.end) return daisycap(from, g.to, span(t, g.start, g.end), g.drag);
		from = g.to;
	}
	return from;
}

/** Every click of the system pointer, with what it presses. */
export const CLICKS: { t: number; target: string }[] = [
	{ t: T.source, target: 'window' },
	{ t: T.record, target: 'record' },
	...REC.clicks.map((c) => ({ t: T.rec + c.t, target: 'desk' })),
	{ t: T.stop, target: 'stop' },
	{ t: T.play, target: 'play' },
	{ t: T.cursor, target: 'cursor' },
	{ t: T.off, target: 'off' },
	{ t: T.smooth, target: 'smooth' },
	{ t: T.background, target: 'background' },
	{ t: T.swatches[0], target: 'wp:ocean' },
	{ t: T.swatches[1], target: 'wp:grape' },
	{ t: T.swatches[2], target: 'wp:lagoon' },
	{ t: T.export, target: 'export' },
	{ t: T.go, target: 'go' }
];

/** What the pointer is holding down at `t`: a click for 150 ms, or a slider while dragging it. */
export function pressed(t: number) {
	if (t >= T.padding[0] && t < T.padding[2] + 80) return 'padding';
	if (t >= T.roundness[0] && t < T.roundness[2] + 80) return 'roundness';
	return CLICKS.find((c) => t >= c.t && t < c.t + 150)?.target ?? null;
}

export type Caption = { start: number; end: number; words: { text: string; at: number }[] };

/** Spreads the words across [start, end], giving longer words more time. */
function caption(text: string, start: number, end: number): Caption {
	const parts = text.split(' ');
	const weights = parts.map((w) => w.length + 2);
	const total = weights.reduce((a, b) => a + b, 0);
	let at = start;
	const words = parts.map((w, i) => {
		const word = { text: w, at };
		at += ((end - start) * weights[i]) / total;
		return word;
	});
	return { start, end, words };
}

export const CAPTIONS = [
	caption('Pick a window and press record.', 200, 1500),
	caption('Click around like you normally would.', 4300, 5900),
	caption('It zooms in on what you click.', 8700, 9500),
	caption('Shaky moves come out smooth.', 13_500, 15_300),
	caption('Pick a background and frame the shot.', 17_500, 19_500),
	caption('Then export, faster than real time.', 23_300, 24_700)
];

export type Cue = { t: number; sound: Sound; variant?: number };

const cue = (t: number, sound: Sound, variant?: number): Cue => ({ t, sound, variant });
const ticks = (from: number, to: number, start: number, end: number, step: number) =>
	Array.from({ length: Math.round((to - from) / step) }, (_, i) => {
		const v = from + (i + 0.5) * step;
		const p = (v - from) / (to - from);
		// Inverse of easeInOut, so each tick lands where the value crosses a step.
		const q = p < 0.5 ? Math.cbrt(p / 4) : 1 - Math.cbrt(2 * (1 - p)) / 2;
		return cue(lerp(start, end, q), 'tick', i);
	});

export const CUES: Cue[] = [
	...CLICKS.map((c, i) => cue(c.t, 'click', i)),
	cue(T.hide, 'whoosh'),
	...T.count.map((t, i) => cue(t, 'beep', i)),
	cue(T.rec, 'rec'),
	cue(T.editor, 'whoosh'),
	...T.pops.map((t, i) => cue(t, 'pop', i)),
	cue(T.playback + 650, 'whoosh'),
	cue(T.playback + 2500, 'whoosh'),
	...ticks(9, 14, T.padding[1], T.padding[2], 1),
	...ticks(20, 32, T.roundness[1], T.roundness[2], 2),
	cue(T.sheet, 'pop'),
	cue(T.progress[0], 'sweep'),
	cue(T.done, 'chime')
].sort((a, b) => a.t - b.t);
