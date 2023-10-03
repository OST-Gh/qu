///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
//! [I hate myself, for making documentation.]
//!
//! ### How Quing works.
//! Quing works around 2 central structures:
//! - A [`Track`]
//! - A [`Playlist`] (grouping of [`Tracks`], with additional data)
//!
//! [`Track`]: in_out::Track
//! [`Tracks`]: in_out::Track
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
use std::{
	cell::OnceCell,
	env::args,
	panic::{ self, PanicInfo },
	path::{ MAIN_SEPARATOR_STR, PathBuf },
	time::{ Duration, Instant },
	env::{ VarError, var },
	io::{ stdout, stdin, BufRead },
};
use crossterm::{
	cursor::Hide,
	execute,
	tty::IsTty, // io::IsTerminal?
	terminal::{ enable_raw_mode, disable_raw_mode },
	style::{
		SetForegroundColor,
		Color,
	},
};
use crossbeam_channel::RecvTimeoutError;
use rodio::Sink;
use in_out::{
	Bundle,
	Flags,
	Playlist,
	UnwrappedPlaylist,
	Instruction,
};
use echo::{ exit, clear };
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
/// A module for handling and interacting with external devices.
/// A collection of file related structures, or implementations.
mod in_out;

/// A collection of functions that are used repeatedly to display certain sequences.
mod echo;
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
/// Constant signal [`Duration`] (tick rate). [250 milliseconds]
///
/// Every time related operation is tackted after this constant.\
const TICK: Duration = Duration::from_millis(250);
/// This is a default message that is used when a [`Sender`] or [`Receiver`] has hung up the connection.
///
/// [`Sender`]: crossbeam_channel::Sender
/// [`Receiver`]: crossbeam_channel::Receiver
const DISCONNECTED: &'static str = "DISCONNECTED CHANNEL";
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
#[macro_export]
/// A macro for general interaction with Standard-Out.
///
/// This macro is, in a general sense, just a fancier [`println`] macro, which also is more tailored towards [raw-mode].
///
/// [raw-mode]: crossterm::terminal#raw-mode
macro_rules! log {
	($($value: expr),*; $message: literal $($why: ident)+ $(; $($retaliation: tt)+)?) => {
		{
			print!(
				concat!("\rError whilst ", $message, ';')
				$(, $value)*
			);
			$(print!(" '{}'", format!("{}", $why).replace('\n', "\r\n"));)+
			print!("\n\0");
			$($($retaliation)+)?
		}
	};

}
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
/// Format a text representation of a path into an absolute path.
///
/// This recursive function is used for unexpanded shell(zsh based) expressions, on a call site, and songs' file fields.
/// It can currently only expand environment variables, which might recurs.
fn fmt_path(path: impl AsRef<str>) -> PathBuf {
	fn expand(name: &str) -> Result<String, VarError> {
		let mut buffer = Vec::new();
		for part in var(if name.starts_with('$') { expand(&name[1..])? } else { String::from(name) })?
			.split(MAIN_SEPARATOR_STR)
			.map(|part| if part.starts_with('$') { expand(&part[1..]) } else { Ok(String::from(part)) })
		{ buffer.push(part?) }
		Ok(buffer.join(MAIN_SEPARATOR_STR))
	}

	let path = path.as_ref();
	PathBuf::from(
		path
			.split(MAIN_SEPARATOR_STR)
			.enumerate()
			.filter_map(|(index, part)|
				match match part {
					"~" if index == 0 => expand("HOME"),
					_ if part.starts_with('$') => expand(&part[1..]),
					_ => return Some(String::from(part)),
				} {
					Ok(part) => Some(part),
					Err(why) => log!(part; "expanding [{}] to a path" why; None)
				}
			)
			.collect::<Vec<String>>()
			.join(MAIN_SEPARATOR_STR)
	)
		.canonicalize()
		.unwrap_or_else(|why| log!(path; "canonicalising [{}]" why; PathBuf::new()))
}

fn panic_handle(info: &PanicInfo) {
	let payload = info.payload();
	let panic = payload
		.downcast_ref::<&str>()
		.map(|slice| String::from(*slice))
		.xor(
			payload
				.downcast_ref::<String>()
				.map(String::from)
		)
		.unwrap();
	let panic = panic
		.splitn(2, "  ")
		.collect::<Vec<&str>>();
	let message = unsafe { panic.get_unchecked(0) };
	let reason = panic
		.get(1)
		.unwrap_or(&"NO_DISPLAYABLE_INFORMATION")
		.replace('\n', "\r\n");
	print!("\rAn error occurred whilst attempting to {message}; '{reason}'\n\0");
	exit();
	if let Err(why) = disable_raw_mode() { log!(; "disabling raw-mode" why) }
}

fn main() {
	panic::set_hook(Box::new(panic_handle));

	let is_tty = stdin().is_tty();
	let mut arguments: Vec<String> = args()
		.skip(1) // skips the executable path (e.g.: //bin/{bin-name})
		.collect();
	if !is_tty {
		arguments.reserve(16);
		arguments.extend(
			stdin()
				.lock()
				.lines()
				.filter_map(Result::ok)
				.map(String::from)
		)
	};
	if let None = arguments.first() { panic!("get the program arguments  no arguments given") }
	let (flags, files) = Flags::separate_from(arguments);
	if !flags.should_spawn_headless() && is_tty {
		if let Err(why) = enable_raw_mode() { panic!("enable the raw mode of the current terminal  {why}") }
		if let Err(why) = execute!(stdout(),
			Hide,
			SetForegroundColor(Color::Yellow),
		) { log!(; "setting the terminal style" why) }
	}

	if flags.should_print_version() { print!(concat!('\r', env!("CARGO_PKG_NAME"), " on version ", env!("CARGO_PKG_VERSION"), " by ", env!("CARGO_PKG_AUTHORS"), ".\n\0")) }

	let mut lists = Playlist::from_paths_with_flags(files, &flags);
	let initialisable_bundle = OnceCell::new(); // expensive operation only executed if no err.

	let lists_length = lists.len();
	let mut lists_index = 0;
	while lists_index < lists_length {
		let old_lists_index = lists_index;
		let list = unsafe { lists.get_unchecked_mut(old_lists_index) };

		list.shuffle_song();
		let mut list = match UnwrappedPlaylist::try_from(&*list) {
			Ok(unwrapped) => unwrapped,
			Err((path, why)) => log!(path; "loading [{}]" why; break),
		};

		let bundle = initialisable_bundle.get_or_init(|| Bundle::with(is_tty || flags.should_spawn_headless()));

		if list.is_empty() { lists_index += 1 }
		match list.play(bundle) {
			Some(Instruction::ExitQuit) => break,
			Some(Instruction::NextNext) => lists_index += 1,
			Some(Instruction::BackBack) => lists_index -= (lists_index > 0) as usize,
			_ => { /* handled by play itself */ },
		}
		clear()
	}

	if let Some(controls) = initialisable_bundle
		.into_inner()
		.map(Bundle::take_controls)
		.flatten()
	{
		controls.notify_exit();
		controls.clean_up();
	}
	if !flags.should_spawn_headless() {
		if let Err(why) = disable_raw_mode() { panic!("disable the raw mode of the current terminal  {why}") }
	}
	exit()
}
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
