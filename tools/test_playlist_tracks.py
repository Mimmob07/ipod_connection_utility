import asyncio
import unittest

from playlist_tracks import playlist_details, read_playlist, build_parser, resolve


def song(identifier, title='Song'):
    return {'id': str(identifier), 'type': 'songs',
            'attributes': {'name': title, 'artistName': 'Artist'}}


class PlaylistTests(unittest.TestCase):
    def test_cookie_argument_is_alternative_to_wrapper(self):
        args = build_parser().parse_args(['pl.test', '--cookies-path', '/private/cookies.txt'])
        self.assertEqual(args.cookies_path, '/private/cookies.txt')
        with self.assertRaises(SystemExit):
            build_parser().parse_args(['pl.test'])

    def test_titles_unavailable_tracks_and_safe_folder(self):
        result = playlist_details(
            {'id': 'pl.test', 'attributes': {'name': '../Mix: One', 'curatorName': 'Me/You'}},
            [song(1, 'First'), song(2, 'Second'),
             {'id': 'i.local', 'type': 'library-songs', 'attributes': {}},
             {'id': '3', 'type': 'music-videos', 'attributes': {'name': 'Video'}}], 'us')
        self.assertEqual([track['title'] for track in result['tracks']],
                         ['First', 'Second', 'Unknown Song', 'Video'])
        self.assertEqual(result['tracks'][0]['source'], 'https://music.apple.com/us/song/track/1')
        self.assertIsNone(result['tracks'][0]['unavailable'])
        self.assertIsNotNone(result['tracks'][2]['unavailable'])
        self.assertIsNotNone(result['tracks'][3]['unavailable'])
        self.assertEqual(len(result['folder'].split('/')), 2)
        self.assertTrue(result['folder'].startswith('Playlists/'))

    def test_all_pages_are_expanded_in_order_including_repeat_songs(self):
        class Api:
            storefront = 'gb'
            async def get_playlist(self, identifier):
                return {'data': [{'id': identifier, 'attributes': {'name': 'Mix'},
                    'relationships': {'tracks': {'data': [song(1)], 'next': '/next', 'href': '/tracks'}}}]}

            async def get_extended_api_data(self, next_uri, href):
                self.next_uri, self.href = next_uri, href
                return {'data': [song(2), song(1)]}

        api = Api()
        result = asyncio.run(read_playlist(api, 'pl.test'))
        self.assertEqual([track['id'] for track in result['tracks']], ['1', '2', '1'])
        self.assertIn('/gb/song/', result['tracks'][0]['source'])
        self.assertEqual((api.next_uri, api.href), ('/next', '/tracks'))

    def test_repeated_pagination_fails_instead_of_looping(self):
        class Api:
            storefront = 'us'
            async def get_playlist(self, identifier):
                return {'data': [{'id': identifier, 'relationships': {'tracks': {
                    'data': [song(1)], 'next': '/next', 'href': '/tracks'}}}]}

            async def get_extended_api_data(self, next_uri, href):
                return {'data': [song(2)], 'next': '/next'}

        with self.assertRaisesRegex(ValueError, 'Repeated playlist page'):
            asyncio.run(read_playlist(Api(), 'pl.test'))


if __name__ == '__main__':
    unittest.main()
