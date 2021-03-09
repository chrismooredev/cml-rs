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
    * does not currently support double-quoted completions, or those with variables
  * Matches the terminal name to the currently connected device's prompt
  * Enables some otherwise unavailable/hard-to-remember keyboard shortcuts
    * home/end/delete/ctrl-left/ctrl-right/etc
  * Pipe in multiple commands using stdin to automate terminal sessions
    * highly recommended: use --wait to wait for the next prompt between commands
      * Prefix a line with a tilde `~` to override the --wait flag, or to not wait for a prompt before entering commands. (Enter a backslash before it to input a literal tilde at the beginning of a line)
      * Prefix a line with a string enclosed in graves ``` `my_string` ``` and it will wait for that string instead of a prompt (or timeout before sending)
      * Note that this has the subtle effect of enabling line-buffering. This is used to ensure that the string above could be found. Additionally, this means that the searched string may not span multiple lines.
  * TODO: basic opt-in coloring
    * colorize prompt by (copy prompt, \r, color, print prompt, reset color on enter)
* (TODO) `cmldiff` - Obtains the saved/running config for a device, and compares it to another device's
  

## Authentication:
This library currently expects authentication in the form of environment variables. (Alternative auth mechanisms are welcome to discussion in GH issues)
* `CML_HOST`: An IP address or hostname where the CML instance can be accessed.
* `CML_USER`: The username to sign into CML with
* `CML_PASS64`: A base-64'd version of the user's password (preferred)
* `CML_PASS`: A plaintext version of the user's password (not recommended)
