#!/bin/bash

# source __shell_completion_wrapper.sh
# declare -f _cmlrest_shell_comp cmlrest

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

function _cmlrest_shell_comp() {
	exe="$1"
	word="$2"
	prev_word="$3"

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
		done | ../target/debug/__cmlrest_shell_completion --wordbreaks "$COMP_WORDBREAKS" --exe "$exe" --word "$word" --prev-word "$3"
	)

}


