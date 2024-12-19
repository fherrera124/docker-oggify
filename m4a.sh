#!/bin/ash

set -e

fname="${4} - ${2}.m4a"
fname="${fname//\//-}"
fname="${PATH_DIR}${fname//\//-}"
#fname="${PATH_DIR}tracks/${fname//\//-}"
spotify_id=${1}
title=${2//'\n'/' '}
album=${3//'\n'/' '}
shift 3
artist="$@"
echo "${fname}"

ffmpeg -y -f ogg -i pipe:0 -i ${PATH_DIR}cover.jpg -map 0 -map 1 -c copy -c:a libfdk_aac -c:v:1 mpeg -disposition:v:0 attached_pic -metadata title="${title}" -metadata album="${album}" -metadata artist="${artist}" "${fname}"