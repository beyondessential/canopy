/// Give a fieldless enum an `ALL` constant listing every variant, and fail to
/// compile when the list and the enum disagree.
///
/// Call it as `all_variants!(Kind: First, Second)`. The same list drives an
/// exhaustive `match` over the enum, so a variant missing from the list is a
/// compile error and `ALL` cannot go stale. Tests iterate `ALL` to cover every
/// variant, so adding one fails them until it is handled.
#[macro_export]
macro_rules! all_variants {
	($ty:ident: $($variant:ident),+ $(,)?) => {
		impl $ty {
			/// Every variant, in declaration order.
			pub const ALL: &'static [Self] = &[$(Self::$variant),+];
		}

		const _: fn($ty) = |value| match value {
			$($ty::$variant)|+ => (),
		};
	};
}
