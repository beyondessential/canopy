/// A kind of request about a DNS name: for address records, or for a TLS
/// certificate.
export type DnsNameKind = "addresses" | "certificate";

/// What differs between the two kinds of request about a DNS name. The two are
/// separate features that share infrastructure, so each has its own module on
/// the private API, and the sections for them are one component fed from
/// whichever the kind names.
// spec: DNS#presentation
export const KIND_MODULES = {
	addresses: "dns_names",
	certificate: "certificates",
} as const satisfies Record<DnsNameKind, string>;

export const KIND_HEADINGS: Record<DnsNameKind, string> = {
	addresses: "DNS names",
	certificate: "TLS certificates",
};

/// The kind as a noun phrase, for sentences that name what is being declared or
/// denied.
export const KIND_NOUNS: Record<DnsNameKind, string> = {
	addresses: "addresses",
	certificate: "certificates",
};
