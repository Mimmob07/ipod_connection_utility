"""Name one staged M4A for the library layout and optionally prepare Rockbox artwork (RGB, 500x500, LANCZOS, baseline JPEG, quality 90)."""
import argparse
import json
from io import BytesIO
from pathlib import Path

from mutagen.mp4 import MP4, MP4Cover


def component(value):
    replacements = str.maketrans('/\\:*?"<>|', '／＼：＊？＂＜＞｜')
    text = ''.join('_' if ord(c) < 32 else c for c in str(value)).translate(replacements)
    text = text.strip().rstrip('.')
    text = text.encode('utf-8')[:220].decode('utf-8', errors='ignore')
    return text if text and text not in ('.', '..') else '_'


def prepare(path, playlist_folder=None, rockbox_art=False):
    audio = MP4(path)

    def tag(key, default=''):
        return audio.get(key, [default])[0]

    title = tag('\xa9nam', path.stem)
    artist = tag('\xa9ART', 'Unknown Artist')
    explicit = ' 🅴' if tag('rtng', 0) == 1 else ''
    if playlist_folder:
        # Folder metadata is supplied by gamdl's playlist templates.
        folder = Path(playlist_folder)
        name = component(f'{title} - {artist}{explicit}') + '.m4a'
    else:
        album_artist = tag('aART', artist)
        album = tag('\xa9alb', 'Unknown Album')
        date = str(tag('\xa9day', ''))
        year = date[:4] if date[:4].isdigit() else 'Unknown'
        track, total = tag('trkn', (0, 0))
        disc, discs = tag('disk', (0, 0))
        prefix = f'{disc}-{track}. ' if discs > 1 else f'{track}. ' if total > 1 else ''
        folder = Path('Tracks') / component(album_artist) / component(f'[{year}] {album}')
        name = component(f'{prefix}{title}{explicit}') + '.m4a'

    processed = False
    if rockbox_art and audio.get('covr'):
        from PIL import Image

        with Image.open(BytesIO(audio['covr'][0])) as image:
            image = image.convert('RGB').resize((500, 500), Image.Resampling.LANCZOS)
            output = BytesIO()
            image.save(output, format='JPEG', progressive=False, quality=90)
        audio['covr'] = [MP4Cover(output.getvalue(), imageformat=MP4Cover.FORMAT_JPEG)]
        audio.save()
        processed = True
    return {'relative_path': str(folder / name), 'art_processed': processed,
            'track_id': str(tag('cnID', ''))}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('file', type=Path)
    parser.add_argument('--playlist-folder')
    parser.add_argument('--rockbox-art', action='store_true')
    args = parser.parse_args()
    print(json.dumps(prepare(args.file, args.playlist_folder, args.rockbox_art)))
