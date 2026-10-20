const APPROXIMATE = new Intl.NumberFormat("en", {
	notation: "compact",
	maximumSignificantDigits: 2,
});

/// A count to two significant figures, as in 440, 1.2K or 57K.
export function approximately(n: number): string {
	return APPROXIMATE.format(n);
}
