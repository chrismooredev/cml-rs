#!/bin/bash

# Use as:
#   source __cmlterm_completion_wrapper.sh
#   declare -F __cmlterm_shell_comp cmlterm

# COMP_CWORD - shell funcs
# COMP_LINE
# COMP_POINT
# COMP_TYPE
# COMP_KEY
# COMP_WORDBREAKS - shell funcs
# COMP_WORDS - shell funcs
# COMPREPLY - return variable

# supply COMP_WORDS over stdin ('\0' seperated)
# read COMPREPLY from stdout ('\0' seperated)

function __cmlterm_shell_comp() {
	local exe="$1"
	local word="$2"
	local prev_word="$3"
	local BINNAME="__cmlterm_shell_completion"
	local binpath="$BINNAME"
	if which "./target/debug/$BINNAME" &>/dev/null ; then
		binpath="./target/debug/$BINNAME"
	elif which "../target/debug/$BINNAME" &>/dev/null ; then
		binpath="../target/debug/$BINNAME"
	fi # hopefully it's globally available... not much more we can try

	# re-export it
	export COMP_LINE="$COMP_LINE"
	export COMP_POINT="$COMP_POINT"
	export COMP_TYPE="$COMP_TYPE"
	export COMP_KEY="$COMP_KEY"

	#for i in "${COMP_WORDS[@]}" ; do
	#	echo -n "$i"
	#	echo -ne '\0'
	#done | ../target/debug/__cmlrest_shell_completion --wordbreaks "$COMP_WORDBREAKS" --exe "$exe" --word "$word" --prev-word "$3" | tr '\0' '\n' # read -r -a COMPREPLY -d $'\0'

	readarray -d $'\0' COMPREPLY < <(
		for i in "${COMP_WORDS[@]}" ; do
			echo -n "$i"
			echo -ne '\0'
		done | "$binpath" --wordbreaks "$COMP_WORDBREAKS" --exe "$exe" --word "$word" --prev-word "$3" 2>debug.txt
	)
}

complete -o nospace -o nosort -F __cmlterm_shell_comp cmlterm
