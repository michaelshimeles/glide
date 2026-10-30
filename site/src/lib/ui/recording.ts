import { clamp, lerp, span } from '#lib/ease.js';
import type { Point } from './screen.js';

/** A pointer move in recording time (ms): from wherever the pointer is to `to`. */
export type Move = { start: number; end: number; to: Point };
/** An automatic zoom segment, as on the timeline's Zoom track. */
export type Zoom = { start: number; end: number; at: Point; level: number };
type Spring = { k: number; c: number; m: number };

/** DaisyCap's spring presets (engine/spring.rs): stiffness, damping, mass. */
const SCREEN_FOCUSED: Spring = { k: 200, c: 40, m: 2.25 };
const CURSOR_SMOOTH: Spring = { k: 470, c: 70, m: 3 };
const NEAR_CLICK: Spring = { k: 530, c: 40, m: 1 };

/** From engine/cursor.rs. */
const CLICK_SNAP_MS = 500;
const PRESS_LEAD_MS = 130;
const TILT_WINDOW_MS = 400;
const TILT_GAIN = 0.03;
const CIRCLE_MS = 150;
const CIRCLE_TAIL_MS = 450;

const STEP = 4;

const minimumJerk = (p: number) => p * p * p * (10 - 15 * p + 6 * p * p);

/**
 * A synthetic recording: a hand-moved pointer with clicks, and what DaisyCap's engine makes of it.
 * The smoothed cursor and the camera are simulated with the app's springs once, up front.
 */
export class Recording {
	readonly clicks: { t: number; at: Point }[];
	#smooth: Float32Array[];
	#camera: Float32Array[];

	constructor(
		readonly duration: number,
		readonly start: Point,
		readonly moves: Move[],
		clicks: number[],
		readonly zooms: Zoom[] = [],
		readonly jitter = 9
	) {
		this.clicks = clicks.map((t) => ({ t, at: this.raw(t) }));
		const n = Math.floor(duration / STEP) + 1;
		this.#smooth = [new Float32Array(n), new Float32Array(n)];
		this.#camera = [new Float32Array(n), new Float32Array(n), new Float32Array(n), new Float32Array(n)];

		const cursor = { x: start.x, y: start.y, vx: 0, vy: 0 };
		const first = zooms[0];
		const cam = { z: 0, vz: 0, level: first?.level ?? 1, x: first?.at.x ?? 0, y: first?.at.y ?? 0, vx: 0, vy: 0 };
		const step = (x: number, v: number, target: number, s: Spring) => {
			const nv = v + ((s.k * (target - x) - s.c * v) / s.m) * 0.001;
			return [x + nv * 0.001, nv];
		};

		for (let i = 0, r = 0; i < n; i++) {
			this.#smooth[0][i] = cursor.x;
			this.#smooth[1][i] = cursor.y;
			this.#camera[0][i] = cam.z;
			this.#camera[1][i] = cam.level;
			this.#camera[2][i] = cam.x;
			this.#camera[3][i] = cam.y;
			for (let j = 0; j < STEP; j++, r++) {
				// With smoothing on, the cursor heads straight for a click that's coming up.
				const next = this.clicks.find((c) => c.t >= r && c.t - r <= CLICK_SNAP_MS);
				const target = next?.at ?? this.raw(r);
				const spring = next ? NEAR_CLICK : CURSOR_SMOOTH;
				[cursor.x, cursor.vx] = step(cursor.x, cursor.vx, target.x, spring);
				[cursor.y, cursor.vy] = step(cursor.y, cursor.vy, target.y, spring);

				const zoom = zooms.find((z) => r >= z.start && r < z.end);
				const focus = zoom ?? zooms.findLast((z) => r >= z.start) ?? first;
				[cam.z, cam.vz] = step(cam.z, cam.vz, zoom ? 1 : 0, SCREEN_FOCUSED);
				if (focus) {
					cam.level = focus.level;
					[cam.x, cam.vx] = step(cam.x, cam.vx, focus.at.x, SCREEN_FOCUSED);
					[cam.y, cam.vy] = step(cam.y, cam.vy, focus.at.y, SCREEN_FOCUSED);
				}
			}
		}
	}

	/** Where the hand put the pointer: eased moves that overshoot a little, with some shake. */
	raw(r: number): Point {
		let from = this.start;
		for (const move of this.moves) {
			if (r < move.start) return from;
			if (r < move.end) return this.#hand(from, move.to, span(r, move.start, move.end), r);
			from = move.to;
		}
		return from;
	}

	#hand(a: Point, b: Point, p: number, r: number): Point {
		const dx = b.x - a.x;
		const dy = b.y - a.y;
		const length = Math.hypot(dx, dy) || 1;
		const q = minimumJerk(p) + 0.05 * Math.sin(Math.PI * clamp((p - 0.55) / 0.45));
		const shake = Math.sin(Math.PI * p) * this.jitter;
		const across = shake * (0.6 * Math.sin(r / 23) + 0.4 * Math.sin(r / 13 + 1.7) + 0.25 * Math.sin(r / 7.7 + 0.3));
		const along = shake * 0.35 * Math.sin(r / 19 + 2.2);
		return {
			x: a.x + dx * q + (-dy / length) * across + (dx / length) * along,
			y: a.y + dy * q + (dx / length) * across + (dy / length) * along
		};
	}

	#sample(track: Float32Array, r: number) {
		const f = clamp(r, 0, this.duration) / STEP;
		const i = Math.floor(f);
		return lerp(track[i], track[Math.min(i + 1, track.length - 1)], f - i);
	}

	/** The cursor DaisyCap draws with smoothing on. */
	smooth(r: number): Point {
		return { x: this.#sample(this.#smooth[0], r), y: this.#sample(this.#smooth[1], r) };
	}

	/** Tilt in degrees from recent horizontal travel, at the default strength of 0.5. */
	tilt(r: number) {
		const travel = this.smooth(r).x - this.smooth(r - TILT_WINDOW_MS).x;
		return clamp(travel * TILT_GAIN, -20, 20);
	}

	/** The cursor shrinks while the button is down. */
	pressing(r: number) {
		return this.clicks.some((c) => r >= c.t - PRESS_LEAD_MS && r < c.t + 90);
	}

	/** Zoom progress, level, the point it centres, and motion blur from how fast the camera moves. */
	camera(r: number) {
		const [z, level, x, y] = this.#camera.map((track) => this.#sample(track, r));
		const [z2, , x2, y2] = this.#camera.map((track) => this.#sample(track, r + 16));
		const scale = 1 + (level - 1) * z;
		const speed = Math.abs(z2 - z) * 90 + Math.hypot(x2 - x, y2 - y) * scale * 0.09 * clamp(z * 2);
		return { zoom: z, level, focus: { x, y }, blur: Math.min(3, speed) };
	}

	/** Click circles alive at `r`: a soft disc that grows and fades (engine/cursor.rs, circle_effects). */
	circles(r: number, size = 1.5) {
		return this.clicks
			.filter((c) => r >= c.t && r < c.t + CIRCLE_TAIL_MS)
			.map((c) => {
				const p = (r - c.t) / CIRCLE_MS;
				const alpha = 0.6 * Math.min(span(p, 0, 0.3), 1 - span(p, 0.5, 1.6));
				const radius = 16 * size * lerp(0.2, 3.5, 1 - (1 - Math.min(1, p / 1.3)) ** 2);
				return { at: c.at, alpha, radius, blur: 20 * span(p, 0.8, 2.4) };
			})
			.filter((fx) => fx.alpha > 0.002);
	}
}
