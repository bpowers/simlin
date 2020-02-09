// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { OutgoingHttpHeaders, ServerResponse } from 'http';

import { Response } from 'express';

export function interceptWriteHeaders(res: Response, callback: (statusCode: number) => void): void {
  // eslint-disable-next-line @typescript-eslint/unbound-method
  const realWriteHead = res.writeHead;

  // eslint-disable-next-line
  // @ts-ignore
  // eslint-disable-next-line @typescript-eslint/unbound-method
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
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    realWriteHead.apply(res, args as any);

    // eslint-disable-next-line @typescript-eslint/ban-ts-ignore
    // @ts-ignore
    return this;
  };
}
