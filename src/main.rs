#![deny(
	absolute_paths_not_starting_with_crate,
	keyword_idents,
	macro_use_extern_crate,
	meta_variable_misuse,
	missing_abi,
	missing_copy_implementations,
	non_ascii_idents,
	nonstandard_style,
	noop_method_call,
	pointer_structural_match,
	private_in_public,
	rust_2018_idioms,
	unused_qualifications
)]
#![warn(clippy::pedantic)]
#![forbid(unsafe_code)]

use std::fs::FileType;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use bytesize::ByteSize;
use walkdir::WalkDir;

trait IoResultExt {
	type Ok;

	fn unwrap_io(self, operation: &str, path: &Path) -> Self::Ok;
	fn unwrap_io_lazy<P: AsRef<Path>>(self, operation: &str, path: impl FnOnce() -> P) -> Self::Ok;
}

impl<T, E> IoResultExt for Result<T, E>
where
	E: std::fmt::Display,
{
	type Ok = T;

	#[track_caller]
	fn unwrap_io(self, operation: &str, path: &Path) -> T {
		// Not using `unwrap_or_else` to preserve caller location.
		match self {
			Ok(inner) => inner,
			Err(error) => {
				panic!("error {operation} {path:?}: {error}");
			}
		}
	}

	#[track_caller]
	fn unwrap_io_lazy<P: AsRef<Path>>(self, operation: &str, path: impl FnOnce() -> P) -> T {
		// Ditto.
		match self {
			Ok(inner) => inner,
			Err(error) => {
				panic!("error {operation} {:?}: {error}", path().as_ref());
			}
		}
	}
}

/// Delete least-recently used files to limit a directory to a specified size.
#[derive(argh::FromArgs)]
struct Args {
	/// don't actually delete anything
	#[argh(switch, short = 'd')]
	dry_run: bool,
	/// don't delete parent directories if we empty them
	///
	/// Enabling this will leave a "skeleton" of container directories,
	/// hence the default behavior of deleting them for cleanliness.
	#[argh(switch, short = 'k')]
	keep_parents: bool,
	/// the size to limit the directory to
	///
	/// Files will be deleted until this size is reached.
	#[argh(option, short = 's')]
	size: ByteSize,
	/// the directory to process
	#[argh(positional)]
	directory: PathBuf,
}

fn counted_file_type(ty: FileType) -> bool {
	ty.is_file() || ty.is_symlink()
}

fn main() {
	let Args {
		dry_run,
		keep_parents,
		size: ByteSize(goal),
		directory,
	} = argh::from_env();

	let mut size: u64 = WalkDir::new(&directory)
		.min_depth(1)
		.into_iter()
		.map(|entry| entry.unwrap_io("walking", &directory))
		.filter(|entry| counted_file_type(entry.file_type()))
		.map(|entry| {
			entry
				.metadata()
				.unwrap_io_lazy("getting metadata of", || entry.path())
				.size()
		})
		.sum();

	eprintln!("initial size is {}", ByteSize(size));
	if size <= goal {
		eprintln!("no need to delete anything, exiting");
		return;
	}

	let action = if dry_run { "would delete" } else { "deleting" };

	for file in WalkDir::new(&directory).min_depth(1).sort_by_key(|entry| {
		entry
			.metadata()
			.unwrap_io_lazy("getting metadata on", || entry.path())
			.mtime()
	}) {
		let file = file.unwrap_io("reading", &directory);

		if file
			.metadata()
			.unwrap_io_lazy("getting metadata on", || file.path())
			.is_dir()
		{
			continue;
		}

		let path = file.path();
		size -= file
			.metadata()
			.unwrap_io("getting metadata of", path)
			.size();
		eprintln!("{action} {path:?}, size is now {}", ByteSize(size));
		if !dry_run {
			std::fs::remove_file(path).unwrap_io("deleting", path);

			if !keep_parents {
				remove_empty_ancestors(path, &directory);
			}
		}

		if size <= goal {
			eprintln!("size is now under limit, exiting");
			break;
		}
	}
}

fn remove_empty_ancestors(path: &Path, within: &Path) {
	for ancestor in path.ancestors().skip(1) {
		if !ancestor.starts_with(within) {
			break;
		}

		// This approach suits this subroutine because this is a secondary, non-critical part of the functionality.
		// The error case includes "directory not empty", which is a termination condition regardless.
		if std::fs::remove_dir(ancestor).is_err() {
			break;
		}
		eprintln!("deleted empty ancestor {path:?}");
	}
}
