# oggify
Download Spotify tracks to Ogg Vorbis (with a premium account).

This program uses [librespot](https://github.com/librespot-org/librespot),
and as such, requires a Spotify Premium account to use.
It supports downloading single tracks and episodes, but also entire playlists, albums and shows.

## Usage
First, you need to build the Docker image:
```
docker build -t oggify .
```
To download a number of links as `<artist(s)> - <title>.ogg`, run
```
docker run --rm -i -v "$(pwd)":/data oggify --ACCESS-TOKEN "..." < link_list
```
Oggify reads from standard input and looks for a URL or URI in each line,
and checks whether it is a valid Spotify media link. If it is not valid, it will be ignored.

The two formats are those you get with the menu items
"Share → Copy <Media> Link" or "Share → Copy <Media> URI" in the Spotify client,
for example
`open.spotify.com/track/1xPQDRSXDN5QJWm7qHg5Ku`
or
`spotify:track:1xPQDRSXDN5QJWm7qHg5Ku`.

Once you close the standard input or write `"done"` into it,
it will start downloading all tracks and episodes in order of input
into the folder named "tracks".

### File as input
A second form of invocation of oggify is
```
docker run --rm -i -v "$(pwd)":/data oggify < list.txt
) < list.txt



curl -X POST   https://accounts.spotify.com/api/token -d grant_type=refresh_token -d refresh_token=REFRESH_TOKEN -d client_id=CLIENT_ID -d client_secret=CLIENT_SECRET 
