#!/bin/ash

set -e

fname="${4} - ${2}.ogg"
fname="${PATH_DIR}${fname//\//-}"
cat > "${fname}"
{
	echo "SPOTIFY_ID=${1}"
	echo "TITLE=${2//'\n'/' '}"
	echo "ALBUM=${3//'\n'/' '}"
	shift 3
	for artist in "$@"; do
		echo "ARTIST=${artist//'\n'/' '}"
	done
} | vorbiscomment -a "${fname}"
echo "${fname}"
