#!/bin/ash

set -e

# https://superuser.com/questions/169151/embed-album-art-in-ogg-through-command-line-in-linux
insert_album_art() {
    local output_file="$1"
    local image_path="$2"
    local image_mime_type="image/jpeg"
    
    # Export existing comments to file.
    local comments_path
    comments_path=$(mktemp -t "tmp.XXXXXXXXXX")
    vorbiscomment --list --raw "${output_file}" >"${comments_path}"

    # Remove existing images
    sed -i -e '/^metadata_block_picture/d' "${comments_path}"

	# Insert cover image from file.

    # metadata_block_picture format.
	# See: https://xiph.org/flac/format.html#metadata_block_picture


    # Create the image with metadata block
    image_with_headers_path=$(mktemp -t "tmp.XXXXXXXXXX")
    local description=""

    # Reset the cache file
    echo -n "" >"${image_with_headers_path}"

    # Picture type <32>
    printf "0: %.8x" 3 | xxd -r -p >>"${image_with_headers_path}"
    # Mime type length <32>
    printf "0: %.8x" "$(echo -n "${image_mime_type}" | wc -c)" | xxd -r -p >>"${image_with_headers_path}"
    # Mime type (n * 8)
    echo -n "${image_mime_type}" >>"${image_with_headers_path}"
    # Description length <32>
    printf "0: %.8x" "$(echo -n "${description}" | wc -c)" | xxd -r -p >>"${image_with_headers_path}"
    # Description (n * 8)
    echo -n "${description}" >>"${image_with_headers_path}"
    # Picture with <32>
    printf "0: %.8x" 0 | xxd -r -p >>"${image_with_headers_path}"
    # Picture height <32>
    printf "0: %.8x" 0 | xxd -r -p >>"${image_with_headers_path}"
    # Picture color depth <32>
    printf "0: %.8x" 0 | xxd -r -p >>"${image_with_headers_path}"
    # Picture color count <32>
    printf "0: %.8x" 0 | xxd -r -p >>"${image_with_headers_path}"
    # Image file size <32>
    printf "0: %.8x" "$(wc -c < "${image_path}")" | xxd -r -p >>"${image_with_headers_path}"
    # Image file
    cat "${image_path}" >>"${image_with_headers_path}"

    # Insert the block into the OGG file
    echo "metadata_block_picture=$(base64 --wrap=0 <"${image_with_headers_path}")" >>"${comments_path}"

    # Update vorbis file comments
    vorbiscomment --write --raw --commentfile "${comments_path}" "${output_file}"

    # Cleanup
    rm "${image_with_headers_path}" "${comments_path}"
}


track_path="${4}"

cat > "${track_path}"
{
	echo "SPOTIFY_ID=${1}"
	echo "TITLE=${2}"
	echo "ALBUM=${3}"
	shift 5
	for artist in "$@"; do
		echo "ARTIST=${artist//'\n'/' '}"
	done
} | vorbiscomment -a "${track_path}"

cover_path=$(mktemp /tmp/image.XXXXXXXXXX.jpg)

wget -q -O "$cover_path" "${5}"

insert_album_art "${track_path}" "$cover_path" 
