/**
 * DaisyCap's built-in wallpapers, ported from `src/render/art.rs`: soft colour blobs over a base
 * colour, crossed by three translucent glass ribbons with a bright rim. Painted on a canvas, so
 * the page ships no wallpaper images.
 */

type Rgb = [number, number, number];
type Blob = [color: Rgb, x: number, y: number, r: number];

const WALLPAPERS = {
	aurora: { name: 'Aurora', base: [16, 22, 58], blobs: [[[60, 214, 190], 0.15, 0.85, 0.7], [[118, 76, 255], 0.85, 0.15, 0.75], [[40, 120, 255], 0.55, 0.65, 0.6], [[255, 120, 200], 0.95, 0.95, 0.45]] },
	sunset: { name: 'Sunset', base: [60, 18, 60], blobs: [[[255, 140, 60], 0.2, 0.9, 0.75], [[255, 70, 120], 0.8, 0.6, 0.7], [[255, 210, 120], 0.5, 1.1, 0.6], [[120, 50, 160], 0.9, 0.05, 0.6]] },
	ocean: { name: 'Ocean', base: [6, 30, 60], blobs: [[[0, 160, 255], 0.1, 0.2, 0.75], [[0, 220, 200], 0.9, 0.9, 0.7], [[30, 90, 200], 0.6, 0.4, 0.6], [[120, 200, 255], 0.3, 0.95, 0.45]] },
	mint: { name: 'Mint', base: [210, 245, 230], blobs: [[[120, 230, 190], 0.1, 0.9, 0.7], [[170, 220, 255], 0.9, 0.1, 0.7], [[250, 255, 220], 0.6, 0.6, 0.6], [[140, 255, 220], 0.95, 0.95, 0.5]] },
	grape: { name: 'Grape', base: [28, 10, 48], blobs: [[[170, 60, 255], 0.2, 0.3, 0.7], [[255, 60, 180], 0.8, 0.8, 0.7], [[80, 40, 200], 0.9, 0.1, 0.6], [[255, 150, 230], 0.1, 0.95, 0.45]] },
	dawn: { name: 'Dawn', base: [250, 225, 215], blobs: [[[255, 180, 160], 0.1, 0.1, 0.8], [[200, 190, 255], 0.9, 0.8, 0.8], [[255, 230, 180], 0.5, 0.95, 0.6], [[255, 200, 230], 0.95, 0.15, 0.55]] },
	ember: { name: 'Ember', base: [30, 8, 8], blobs: [[[255, 90, 30], 0.15, 0.85, 0.7], [[255, 40, 60], 0.85, 0.25, 0.65], [[255, 180, 60], 0.55, 0.55, 0.5], [[120, 20, 40], 0.9, 0.95, 0.6]] },
	lagoon: { name: 'Lagoon', base: [10, 50, 50], blobs: [[[30, 200, 160], 0.2, 0.2, 0.75], [[20, 120, 200], 0.85, 0.75, 0.7], [[160, 255, 210], 0.6, 0.15, 0.5], [[10, 80, 120], 0.1, 0.95, 0.6]] },
	graphite: { name: 'Graphite', base: [28, 29, 33], blobs: [[[70, 72, 82], 0.2, 0.2, 0.8], [[45, 47, 55], 0.85, 0.85, 0.8], [[95, 98, 110], 0.7, 0.2, 0.5], [[20, 20, 24], 0.2, 0.95, 0.6]] },
	snow: { name: 'Snow', base: [236, 238, 243], blobs: [[[255, 255, 255], 0.2, 0.2, 0.8], [[214, 220, 232], 0.85, 0.85, 0.8], [[245, 246, 250], 0.7, 0.2, 0.5], [[225, 230, 240], 0.15, 0.9, 0.6]] }
} satisfies Record<string, { name: string; base: Rgb; blobs: Blob[] }>;

export type WallpaperId = keyof typeof WALLPAPERS;

/** In the app's order, which also sets each wallpaper's ribbon phase. */
export const WALLPAPER_IDS = Object.keys(WALLPAPERS) as WallpaperId[];

const TAU = Math.PI * 2;

function paint(id: WallpaperId, w: number, h: number) {
	const { base, blobs } = WALLPAPERS[id];
	const index = WALLPAPER_IDS.indexOf(id);
	const light = base[0] + base[1] + base[2] > 3 * 160;
	const target = light ? 0 : 255;
	const glass = 0.16;
	const phase = index * 0.37;

	const centres = new Float32Array(w * 3);
	const halves = new Float32Array(w * 3);
	for (let x = 0; x < w; x++) {
		const u = x / w;
		for (let k = 0; k < 3; k++) {
			const p = phase + k * 1.9;
			centres[x * 3 + k] = 0.22 + 0.28 * k + 0.09 * Math.sin(TAU * (0.55 * u + p)) + 0.035 * Math.sin(TAU * (1.6 * u + 1.3 * p));
			halves[x * 3 + k] = 0.055 + 0.025 * Math.sin(TAU * (0.8 * u + 0.7 * p));
		}
	}

	const image = new ImageData(w, h);
	const out = image.data;
	const aspect = w / h;
	const c = [0, 0, 0];
	for (let y = 0; y < h; y++) {
		const v = y / h;
		let seed = (0x9e3779b9 ^ Math.imul(y, 0x85ebca6b)) >>> 0;
		for (let x = 0; x < w; x++) {
			const u = x / w;
			c[0] = base[0];
			c[1] = base[1];
			c[2] = base[2];
			for (const [col, bx, by, r] of blobs) {
				const dx = (u - bx) * aspect;
				const dy = v - by;
				const weight = Math.exp((-(dx * dx + dy * dy) / (r * r)) * 2.2);
				c[0] += (col[0] - c[0]) * weight;
				c[1] += (col[1] - c[1]) * weight;
				c[2] += (col[2] - c[2]) * weight;
			}
			for (let k = 0; k < 3; k++) {
				const centre = centres[x * 3 + k];
				const half = halves[x * 3 + k];
				const inside = Math.min(1, Math.max(0, 0.5 - (Math.abs(v - centre) - half) * h));
				const rim = Math.min(1, Math.max(0, 1 - Math.abs((v - (centre - half)) * h - 2) / 2.5));
				if (inside > 0 || rim > 0) {
					for (let i = 0; i < 3; i++) {
						c[i] += (target - c[i]) * inside * glass;
						c[i] += (255 - c[i]) * rim * 0.38;
					}
				}
			}
			seed ^= seed << 13;
			seed >>>= 0;
			seed ^= seed >>> 17;
			seed ^= seed << 5;
			seed >>>= 0;
			const n = (seed & 0xff) / 255 - 0.5;
			const o = (y * w + x) * 4;
			out[o] = c[0] + n * 1.5;
			out[o + 1] = c[1] + n * 1.5;
			out[o + 2] = c[2] + n * 1.5;
			out[o + 3] = 255;
		}
	}
	return image;
}

const cache = new Map<string, ImageData>();

/** The wallpaper at `w`×`h` pixels. Blobs stay round, so the size sets the aspect ratio too. */
export function wallpaper(id: WallpaperId, w: number, h: number) {
	const key = `${id}:${w}x${h}`;
	let image = cache.get(key);
	if (!image) {
		image = paint(id, w, h);
		cache.set(key, image);
	}
	return image;
}

/** A CSS stand-in with the same base and blobs, shown until the canvas paints. */
export function fallback(id: WallpaperId) {
	const { base, blobs } = WALLPAPERS[id];
	const layers = blobs.map(([[r, g, b], x, y, radius]) => `radial-gradient(circle at ${x * 100}% ${y * 100}%, rgb(${r} ${g} ${b}), transparent ${radius * 90}%)`);
	return `${layers.reverse().join(', ')}, rgb(${base.join(' ')})`;
}

export const wallpaperName = (id: WallpaperId) => WALLPAPERS[id].name;
