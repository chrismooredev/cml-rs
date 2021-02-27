# CML Utilties
### for [Cisco Modeling Labs / VIRL2](https://www.cisco.com/c/en/us/products/cloud-systems-management/modeling-labs/index.html)

This suite of Rust crates provides CLI-based utilities to make managing and automating your CML instance easier.


## Crates
* `cml` - Provides a type-safe interface to CML's REST API
  * Still a Work-in-progress, tested PRs welcome for new endpoints
	* The built-in documentation explorer cannot be trusted. Always use real API output when creating type definitions.
  * WIP: bash commandline completion for REST endpoints
* `cmlrest` - A command-line interface to CML's REST API
  * Supports outputting human-formatted, or JSON data
* `cmlterm` - A command-line terminal for CML
  * Allows entering a device directly without an intermediary shell
  * WIP: bash commandline completion for lab/device IDs/names
  * Matches the terminal name to the currently connected device's prompt
  * Enables some otherwise unavailable/hard-to-remember keyboard shortcuts
    * home/end/delete/ctrl-left/ctrl-right/etc
  * Pipe in multiple commands using stdin to automate terminal sessions
    * highly recommended: use --wait to wait for the next prompt between commands
  * TODO: custom menu accessible by ctrl-q ??
  * TODO: basic opt-in coloring
    * colorize prompt by (copy prompt, \r, color, print prompt, reset color on enter)
* (TODO) `cmldiff` - Obtains the saved/running config for a device, and compares it to another device's
  

## Authentication:
This library currently expects authentication in the form of environment variables. (Alternative auth mechanisms are welcome to discussion in GH issues)
* `CML_HOST`: An IP address or hostname where the CML instance can be accessed.
* `CML_USER`: The username to sign into CML with
* `CML_PASS64`: A base-64'd version of the user's password (preferred)
* `CML_PASS`: A plaintext version of the user's password (not recommended)
