// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { OutgoingHttpHeaders, ServerResponse } from 'http';

import { Response } from 'express';

export function interceptWriteHeaders(res: Response, callback: (statusCode: number) => void): void {
  const realWriteHead = res.writeHead;

  // eslint-disable-next-line
  // @ts-ignore
  res.writeHead = (
    statusCode: number,
    reasonOrHeaders?: string | OutgoingHttpHeaders,
    headers?: OutgoingHttpHeaders,
  ): ServerResponse => {
    callback(statusCode);

    // ensure arguments.length is right
    const args: [number, (string | OutgoingHttpHeaders | undefined)?, (OutgoingHttpHeaders | undefined)?] = [
      statusCode,
    ];
    if (reasonOrHeaders !== undefined) {
      args.push(reasonOrHeaders);
      if (headers !== undefined) {
        args.push(headers);
      }
    }

    // TODO: remove this any cast in the future -- for now,
    // typescript can't quite handle the insanity of Node's
    // writeHead's signature
    realWriteHead.apply(res, args as any);

    // eslint-disable-next-line @typescript-eslint/ban-ts-comment
    // @ts-ignore
    return this as ServerResponse;
  };
}
