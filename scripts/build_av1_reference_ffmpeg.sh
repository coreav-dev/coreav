#!/usr/bin/env bash
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "$HERE/oracle-ffmpeg.env"
ARCHIVE_FILE=${1:?pass the pinned source archive}
SVT_PREFIX=${2:?pass the verified SVT-AV1 installation}
DEST=${3:?pass an absent output directory}
[[ ! -e "$DEST" ]]
echo "f02fc0809a13f37e3d4e1f74254d07e035a5ef01a6d91e727b915b1764d97540  $ARCHIVE_FILE" | sha256sum --check --strict
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
tar -xzf "$ARCHIVE_FILE" -C "$WORK" --strip-components=1
mkdir "$WORK/pkgconfig"
sed "s|^prefix=.*|prefix=$SVT_PREFIX|" "$SVT_PREFIX/lib/pkgconfig/SvtAv1Enc.pc" > "$WORK/pkgconfig/SvtAv1Enc.pc"
cd "$WORK"
export PKG_CONFIG_LIBDIR="$WORK/pkgconfig"
./configure --prefix="$DEST" --cc=musl-gcc --disable-everything --disable-autodetect \
    --disable-doc --disable-network --disable-ffplay --enable-libsvtav1 \
    --enable-decoder=h264 --enable-encoder=libsvtav1,rawvideo \
    --enable-parser=h264 --enable-demuxer=mov --enable-muxer=mp4,rawvideo,null \
    --enable-protocol=file,pipe --enable-filter=null,format,scale \
    --pkg-config-flags=--static --extra-cflags="-O3 -I$SVT_PREFIX/include/svt-av1" \
    --extra-ldflags="-static -L$SVT_PREFIX/lib -Wl,-z,stack-size=2097152" --extra-libs='-lpthread -lm' \
    --extra-version="${BUILD#n$VERSION-}" > build.log 2>&1
make -j4 ffmpeg ffprobe >> build.log 2>&1
mkdir -p "$DEST/bin"
cp ffmpeg ffprobe "$DEST/bin/"
rm -rf "$DEST/share/ffmpeg/examples"
"$DEST/bin/ffmpeg" -version > "$DEST/version.txt"
sha256sum "$DEST/bin/ffmpeg" "$DEST/bin/ffprobe" "$SVT_PREFIX/lib/libSvtAv1Enc.a" > "$DEST/identities.txt"
