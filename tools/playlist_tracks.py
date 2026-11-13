"""Resolve playlist metadata with the installed gamdl API; never download audio.

Only public track metadata reaches stdout. Authentication stays inside gamdl's
existing loopback wrapper client; tokens and exception bodies are never printed.
"""
import argparse
import asyncio
import json
import logging
from pathlib import Path


def component(value):
    replacements = str.maketrans('/\\:*?"<>|', '／＼：＊？＂＜＞｜')
    text = ''.join('_' if ord(c) < 32 else c for c in str(value)).translate(replacements)
    text = text.strip().rstrip('.')
    text = text.encode('utf-8')[:220].decode('utf-8', errors='ignore')
    return text if text and text not in ('.', '..') else '_'


def playlist_details(metadata, tracks, storefront):
    attributes = metadata.get('attributes', {})
    title = attributes.get('name') or 'Unknown Playlist'
    curator = attributes.get('curatorName') or 'Unknown Curator'
    result = []
    for track in tracks:
        tags = track.get('attributes', {})
        play = tags.get('playParams', {})
        identifier = str(play.get('catalogId') or track.get('id') or '')
        reason = None
        if track.get('type') not in ('songs', 'library-songs'):
            reason = 'Not an audio song'
        elif not identifier.isascii() or not identifier.isdigit():
            reason = 'No downloadable catalog song'
        result.append({
            'id': identifier,
            'source': f'https://music.apple.com/{storefront}/song/track/{identifier}' if not reason else '',
            'title': tags.get('name') or 'Unknown Song',
            'artist': tags.get('artistName') or 'Unknown Artist',
            'unavailable': reason,
        })
    return {
        'id': str(metadata.get('id', '')),
        'title': title,
        'folder': str(Path('Playlists') / component(f'{title} by {curator}')),
        'tracks': result,
    }


async def read_playlist(api, playlist_id):
    metadata = (await api.get_playlist(playlist_id))['data'][0]
    relationship = metadata.get('relationships', {}).get('tracks', {})
    tracks = list(relationship.get('data', []))
    next_uri = relationship.get('next')
    href = relationship.get('href')
    visited = set()
    while next_uri:
        if next_uri in visited:
            raise ValueError('Repeated playlist page')
        visited.add(next_uri)
        page = await api.get_extended_api_data(next_uri, href)
        tracks.extend(page.get('data', []))
        next_uri = page.get('next')
    return playlist_details(metadata, tracks, api.storefront)


async def resolve(playlist_id, wrapper_url=None, decrypt_port=10020, cookies_path=None):
    import structlog
    from gamdl.api.apple_music import AppleMusicApi


    structlog.configure(wrapper_class=structlog.make_filtering_bound_logger(logging.CRITICAL))
    api = None
    try:
        if cookies_path:
            api = await AppleMusicApi.create_from_netscape_cookies(cookies_path=cookies_path)
            if not api.active_subscription:
                raise ValueError('Active Apple Music subscription required')
        else:
            from gamdl.api.wrapper import WrapperApi
            wrapper = await WrapperApi.create(base_url=wrapper_url, decrypt_host='127.0.0.1',
                                              decrypt_port=decrypt_port)
            api = await AppleMusicApi.create_from_wrapper(wrapper_api=wrapper)
        return await read_playlist(api, playlist_id)
    finally:
        if api:
            await api.client.aclose()
        if not cookies_path and 'wrapper' in locals() and wrapper:
            await wrapper.client.aclose()


def build_parser():
    parser = argparse.ArgumentParser()
    parser.add_argument('playlist_id')
    auth = parser.add_mutually_exclusive_group(required=True)
    auth.add_argument('--wrapper-url')
    auth.add_argument('--cookies-path')
    parser.add_argument('--decrypt-port', type=int, default=10020)
    return parser


if __name__ == '__main__':
    args = build_parser().parse_args()
    try:
        result = asyncio.run(resolve(args.playlist_id, args.wrapper_url, args.decrypt_port, args.cookies_path))
    except Exception as error:
        status = getattr(error, 'status_code', None)
        result = {'error': 'playlist lookup failed', 'status': status if isinstance(status, int) else None}
    print(json.dumps(result))
