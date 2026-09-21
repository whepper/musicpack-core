// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Error mapping: the API never exposes raw C/SQLite/WASM strings, and the
// client maps the typed error codes to human-friendly copy.

export type ApiErrorCode =
  | 'invalid_request'
  | 'unauthorized'
  | 'origin_forbidden'
  | 'not_found'
  | 'unsupported_method'
  | 'scan_already_running'
  | 'bad_range'
  | 'unavailable'
  | 'internal';

export class ApiError extends Error {
  readonly code: ApiErrorCode;
  readonly status: number;
  readonly detail?: string;

  constructor(code: ApiErrorCode, status: number, detail?: string) {
    super(friendlyMessage(code, detail));
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
    this.detail = detail;
  }
}

export class NetworkError extends Error {
  constructor(inner?: unknown) {
    super(
      inner instanceof Error && inner.name === 'AbortError'
        ? 'The request timed out.'
        : 'Cannot reach the MusicPack server.',
    );
    this.name = 'NetworkError';
  }
}

export function friendlyMessage(code: ApiErrorCode, detail?: string): string {
  switch (code) {
    case 'unauthorized':
      return 'Your session has expired. Please sign in again.';
    case 'not_found':
      return 'This item is no longer in the collection.';
    case 'unavailable':
      return 'This audio is unavailable — the package may need a rescan.';
    case 'invalid_request':
      return 'The request was not valid.';
    case 'scan_already_running':
      return 'A library scan is already running.';
    case 'origin_forbidden':
      return 'This server does not allow that origin.';
    case 'internal':
    default:
      return detail || 'Something went wrong on the server.';
  }
}
