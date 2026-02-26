// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { OutgoingHttpHeaders, ServerResponse } from 'http';

import { Response } from 'express';

export function interceptWriteHeaders(res: Response, callback: (statusCode: number) => void): void {
  const realWriteHead = res.writeHead;
  const writeHead = realWriteHead.bind(res);

  // eslint-disable-next-line
  // @ts-ignore
  res.writeHead = (
    statusCode: number,
    reasonOrHeaders?: string | OutgoingHttpHeaders,
    headers?: OutgoingHttpHeaders,
  ): ServerResponse => {
    callback(statusCode);

    if (typeof reasonOrHeaders === 'string') {
      if (headers !== undefined) {
        writeHead(statusCode, reasonOrHeaders, headers);
      } else {
        writeHead(statusCode, reasonOrHeaders);
      }
    } else if (reasonOrHeaders !== undefined) {
      writeHead(statusCode, reasonOrHeaders);
    } else {
      writeHead(statusCode);
    }

    // eslint-disable-next-line @typescript-eslint/ban-ts-comment
    // @ts-ignore
    return this as ServerResponse;
  };
}
