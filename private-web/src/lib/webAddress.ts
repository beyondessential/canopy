/**
 * Whether `address` is a web address, safe to open as a link. Addresses stored
 * before Canopy checked their scheme may hold another one, which would run as
 * script where it is clicked, so anything else is shown as text.
 */
export function isWebAddress(address: string | null | undefined): address is string {
	return !!address && /^https?:\/\//i.test(address);
}
