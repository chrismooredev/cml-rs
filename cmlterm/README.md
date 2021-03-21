
# `cmlterm`

A command-line interface to CML devices.

## Improvements to CML's built in SSH commandline
* Proper handling of home/end/delete/ctrl-left/right/etc keys
* Pipe commands into stdin, clean results from stdout
* Specifying device via commandline (via lab/device IDs or names)
	* Command-line completion (for bash only, see below)
* Sets the current terminal's title to the current device's prompt

## Usage
Detailed help can be accessed with `cmlterm --help`  

Line numbers are inferred to line 0, add `/<line num>` to the end of the device path, if needed.

Open a device by ID
```
cmlterm open /abcde/n2
```
Or, by name:
```
cmlterm open "/3-Site VPN/FW2"
```

## Authentication
Uses three environment variables:

* `CML_HOST`: An IP address or hostname where the CML instance can be accessed.
* `CML_USER`: The username to sign into CML with
* `CML_PASS64`: A base-64'd version of the user's password (preferred)
* `CML_PASS`: A plaintext version of the user's password (not recommended)

## Command-line completion
* Currently `bash`-only, though PRs are encouraged
* Supports plain and single-quoted strings (read: does not complete doubly-quoted strings)
* Needs a special installation step, see installation section below

## Installation
Pre-built binaries are not currently provided on Github.
* If Rust is not yet installed, first install the Rust toolchain: https://rustup.rs/
* `cmlterm` can then be installed with:
	* `cargo install cmlterm --git https://github.com/csm123199/cml-rs --branch main`

For bash autocompletion, you can execute the completion wrapper in your shell. To insert the latest to your .bashrc:
* First find your downloaded completion wrapper. It will be somewhere in
```
~/.cargo/git/checkouts/cml-rs-***/***/cmlterm/__cmlterm_completion_wrapper.sh
```
Then add a `source` it in your .bashrc, if you want to run it for every shell
```
echo 'source "<completion wrapper path>" >> .bashrc
```

## Basic scripting
Commands can be piped in (specifically, input is not an interactive tty), separated by lines. Each line is sent to the device sequentially, and by default, each line waits for the next prompt to show before executing, to prevent commands dissappearing. The script is considered to end after stdin is closed and the next prompt shows.
Piped input supports the following prefixes to change this behavior: (use a backslash before them to use these as literal line prefixes)
* prefixing a line with a tilde `~` will not wait for the next prompt before sending. This may be helpful for automating interactive commands.
* prefixing a line with a grave-quoted string (``` `asd` ```) will wait for the quoted string to appear (specifically within the last 256 bytes of terminal output) before sending the line.
* (NOT YET IMPLEMENTED) a prefix to override the default prompt timeout

Anything dynamic or more interesting than that may need to wrap `cmlterm` inputs/outputs to adjust the input script. If this is done, you may want to visit the (WIP) [`cmlscript`](https://github.com/csm123199/cml-rs/tree/main/cmlscript) binary within this same project for richer device information, and easier terminal multiplexing.

