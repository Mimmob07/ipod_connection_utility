import shutil
import subprocess
import tempfile
import unittest
from io import BytesIO
from pathlib import Path

from mutagen.mp4 import MP4, MP4Cover
from PIL import Image
from prepare_download import component, prepare


@unittest.skipUnless(shutil.which('ffmpeg'), 'ffmpeg required')
class SongPreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.file = Path(self.temp.name) / '123.m4a'
        subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                        'sine=duration=0.1', '-c:a', 'aac', str(self.file)], check=True)
        self.audio = MP4(self.file)
        self.audio['\xa9nam'] = ['Test Song']
        self.audio['\xa9ART'] = ['Test Artist']
        self.audio['aART'] = ['Album Artist']
        self.audio['\xa9alb'] = ['Test Album']
        self.audio['\xa9day'] = ['2026-10-12']
        self.audio['trkn'] = [(3, 12)]
        self.audio['rtng'] = [1]
        self.audio['cnID'] = [123]
        image = BytesIO()
        Image.new('RGBA', (800, 600), (150, 80, 25, 150)).save(image, format='PNG')
        self.original_art = image.getvalue()
        self.audio['covr'] = [MP4Cover(self.original_art, imageformat=MP4Cover.FORMAT_PNG)]
        self.audio.save()

    def audio_hash(self):
        return subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(self.file),
            '-map', '0:a:0', '-c', 'copy', '-f', 'hash', '-hash', 'sha256', '-'])

    def test_rockbox_art_and_track_path_preserve_audio_and_tags(self):
        before = self.audio_hash()
        result = prepare(self.file, rockbox_art=True)
        self.assertEqual(result['relative_path'],
                         'Tracks/Album Artist/[2026] Test Album/3. Test Song 🅴.m4a')
        self.assertEqual(result['track_id'], '123')
        changed = MP4(self.file)
        self.assertEqual(changed['\xa9nam'], ['Test Song'])
        self.assertEqual(changed['covr'][0].imageformat, MP4Cover.FORMAT_JPEG)
        with Image.open(BytesIO(changed['covr'][0])) as art:
            self.assertEqual(art.size, (500, 500))
            self.assertEqual(art.mode, 'RGB')
            self.assertFalse(art.info.get('progressive') or art.info.get('progression'))
        self.assertEqual(before, self.audio_hash())

    def test_playlist_path_and_disabled_artwork(self):
        result = prepare(self.file, 'Playlists/D2 by Test Curator', False)
        self.assertEqual(result['relative_path'],
                         'Playlists/D2 by Test Curator/Test Song - Test Artist 🅴.m4a')
        self.assertEqual(bytes(MP4(self.file)['covr'][0]), self.original_art)

    def test_missing_cover_and_path_components(self):
        del self.audio['covr']
        self.audio.save()
        self.assertFalse(prepare(self.file, rockbox_art=True)['art_processed'])
        for dirty in ('..', '.', '', 'Artist/../../escape', 'slash\\name', '\x00name'):
            clean = component(dirty)
            self.assertNotIn(clean, ('.', '..', ''))
            self.assertNotIn('/', clean)
            self.assertNotIn('\\', clean)
            self.assertNotIn('\x00', clean)


if __name__ == '__main__':
    unittest.main()
