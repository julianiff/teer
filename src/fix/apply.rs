//! Applying proposed hunks to the working tree: each hunk's old text is
//! looked up again in the file as it is on disk now (it must still be
//! there exactly once), then the file is written atomically.
//!
//! Filled in by the core of this feature.
