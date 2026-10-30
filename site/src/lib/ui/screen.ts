/**
 * The recorded screen: the synthetic dashboard that `daisycap synth` draws, laid out at 960×600.
 * Cursor paths aim at its buttons, so their positions live here.
 */
export const SCREEN = { w: 960, h: 600 };

export const CARD = { w: 250, h: 150 };

export const card = (col: number, row: number) => ({ x: 182 + col * 270, y: 50 + row * 172 });

/** Centre of a card's blue button. */
export function button(col: number, row: number) {
	const c = card(col, row);
	return { x: c.x + 44, y: c.y + 126 };
}

export type Point = { x: number; y: number };
