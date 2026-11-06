fn parts(version: &str) -> Option<(u64, u64, u64)> {
  let mut parts = version.split('.').map(str::parse);
  let parsed = (
    parts.next()?.ok()?,
    parts.next()?.ok()?,
    parts.next()?.ok()?,
  );
  parts.next().is_none().then_some(parsed)
}

pub fn valid(version: &str) -> bool {
  parts(version).is_some()
}

/// Whether a release is later than what is running. Anything unreadable is not, so a strange tag
/// can never replace a working binary.
pub fn newer(candidate: &str, current: &str) -> bool {
  matches!((parts(candidate), parts(current)), (Some(candidate), Some(current)) if candidate > current)
}
