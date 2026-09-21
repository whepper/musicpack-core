// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Media Session integration: lock-screen / headset / browser controls plus
// metadata so the client behaves like a proper media app.
import type { QueueItem } from '../state/queue';

export function mediaSessionSupported(): boolean {
  return 'mediaSession' in navigator;
}

export function setMediaMetadata(item: QueueItem | null, artworkUrl?: string): void {
  if (!mediaSessionSupported()) return;
  if (!item) {
    navigator.mediaSession.metadata = null;
    return;
  }
  const artwork: MediaImage[] = artworkUrl
    ? [{ src: artworkUrl, sizes: '512x512', type: 'image/jpeg' }]
    : [];
  navigator.mediaSession.metadata = new MediaMetadata({
    title: item.track.title,
    artist: item.artist,
    album: item.albumTitle,
    artwork,
  });
}

export interface MediaActionHandlers {
  play: () => void;
  pause: () => void;
  next: () => void;
  previous: () => void;
  /** TRACK-relative absolute time (spec semantics of `seekto`). */
  seek: (seconds: number) => void;
  seekBy: (deltaSeconds: number) => void;
}

export function bindMediaActions(handlers: MediaActionHandlers): void {
  if (!mediaSessionSupported()) return;
  const ms = navigator.mediaSession;
  const safe = (action: MediaSessionAction, fn: MediaSessionActionHandler) => {
    try {
      ms.setActionHandler(action, fn);
    } catch {
      /* action unsupported */
    }
  };
  safe('play', handlers.play);
  safe('pause', handlers.pause);
  safe('previoustrack', handlers.previous);
  safe('nexttrack', handlers.next);
  safe('seekto', (details) => {
    if (details.seekTime !== undefined) handlers.seek(details.seekTime);
  });
  safe('seekbackward', () => handlers.seekBy(-10));
  safe('seekforward', () => handlers.seekBy(10));
}

export function setMediaPosition(durationSeconds: number, positionSeconds: number): void {
  if (!mediaSessionSupported()) return;
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) return;
  if (!Number.isFinite(positionSeconds) || positionSeconds < 0) positionSeconds = 0;
  // duration/position describe the CURRENT TRACK (Media Session spec), not
  // an album-spanning timeline; the controller reports track-relative values.
  try {
    navigator.mediaSession.setPositionState?.({ duration: durationSeconds, position: positionSeconds, playbackRate: 1 });
  } catch {
    /* transient invalid position */
  }
}
