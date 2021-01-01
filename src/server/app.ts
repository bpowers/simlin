// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';
import { IncomingMessage, ServerResponse } from 'http';

import * as bodyParser from 'body-parser';
import cookieParser from 'cookie-parser';
import cors from 'cors';
import express from 'express';
import { NextFunction, Request, Response } from 'express';
import helmet from 'helmet';
import favicon from 'serve-favicon';
import { seshcookie } from 'seshcookie';
import * as logger from 'winston';

import { apiRouter } from './api';
import { defined } from '@system-dynamics/core/common';
import { Application } from './application';
import { authn } from './authn';
import { authz } from './authz';
import { createDatabase } from './models/db';
import { redirectToHttps } from './redirect-to-https';
import { requestLogger } from './request-logger';
import { User as UserPb } from './schemas/user_pb';

// redefinition from Helmet, as they don't export it
interface ContentSecurityPolicyDirectiveValueFunction {
  (req: IncomingMessage, res: ServerResponse): string;
}

// redefinition from Helmet, as they don't export it
type ContentSecurityPolicyDirectiveValue = string | ContentSecurityPolicyDirectiveValueFunction;

// redefinition from Helmet, as they don't export it
interface ContentSecurityPolicyDirectives {
  [directiveName: string]: Iterable<ContentSecurityPolicyDirectiveValue>;
}

export async function createApp(): Promise<App> {
  const app = new App();
  await app.setup();

  return app;
}

class App {
  private readonly app: Application;

  constructor() {
    this.app = (express() as any) as Application;
  }

  listen(): void {
    const port = this.app.get('port') as number;
    const server = this.app.listen(port);

    server.on('listening', () => {
      logger.info(`model-service started on http://${this.app.get('host')}:${port}`);
    });
  }

  private loadConfig(): void {
    const setConfig = (filename: string): void => {
      const contents = fs.readFileSync(filename).toString();
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const config = JSON.parse(contents);
      // eslint-disable-next-line prefer-const
      for (let [key, value] of Object.entries(config)) {
        // FML
        if (key === 'port' && value === 'PORT' && process.env.PORT) {
          value = process.env.PORT;
        }
        this.app.set(key, value);
      }
    };
    setConfig('./config/default.json');
    if (process.env.NODE_ENV === 'production') {
      setConfig('./config/production.json');
    }
    // dump all environment variables into our app settings.  enable
    // nested keys with '__', like 'authentication__seshcookie__key'.
    for (const [key, value] of Object.entries(process.env)) {
      if (key.startsWith('npm')) {
        continue;
      }
      let path: string[] = key.split('__');
      if (path.length === 1) {
        this.app.set(key, value);
      } else {
        let component = defined(path[0]);
        path = path.slice(1);
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
        let obj: any = this.app.get(component);
        while (obj && path.length > 1) {
          component = defined(path[0]);
          path = path.slice(1);
          // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
          obj = obj[component];
        }
        if (obj) {
          obj[defined(path[0])] = value;
        }
      }
    }
  }

  private mongoUrl(): string {
    let url = this.app.get('mongodb') as string;
    if (process.env.MODEL_MONGO_USERNAME && process.env.MODEL_MONGO_PASSWORD) {
      const exploded = new URL(url);
      exploded.username = process.env.MODEL_MONGO_USERNAME;
      exploded.password = process.env.MODEL_MONGO_PASSWORD;
      url = exploded.toString();
    }
    return url;
  }

  async setup(): Promise<void> {
    const { combine, timestamp, json } = logger.format;
    logger.configure({
      format: combine(timestamp(), json()),
      transports: [new logger.transports.Console({ level: 'debug' })],
    });

    const oneYearInSeconds = 365 * 24 * 60 * 60;

    this.loadConfig();

    this.app.db = await createDatabase({
      url: this.mongoUrl(),
      backend: this.app.get('database') === 'firestore' ? 'firestore' : 'mongo',
    });

    // put the redirect before the request logger to remove noise
    this.app.use(redirectToHttps);
    this.app.use(requestLogger);
    this.app.use(cookieParser());
    this.app.use(seshcookie(this.app.get('authentication').seshcookie));

    const indexHtml = fs.readFileSync('public/index.html').toString('utf-8');
    const metaTagContentsMatch = indexHtml.match(/http-equiv="Content-Security-Policy"[^>]+/g);
    const additionalScriptSrcs: string[] = [];
    if (metaTagContentsMatch && metaTagContentsMatch.length > 0) {
      const metaTagContents: string = metaTagContentsMatch[0];
      const shasMatch = metaTagContents.match(/sha[^']+/g);
      if (shasMatch) {
        for (const sha of shasMatch) {
          additionalScriptSrcs.push(`'${sha}'`);
        }
      }
    }

    // copy of the default from helmet, with font + style changed from 'https:' to specific google font hosts
    const directives: ContentSecurityPolicyDirectives = {
      'default-src': ["'self'"],
      'base-uri': ["'self'"],
      'block-all-mixed-content': [],
      'font-src': ["'self'", 'data:'],
      'frame-ancestors': ["'self'"],
      'img-src': ["'self'", 'data:', 'https://*.googleusercontent.com'],
      'object-src': ["'none'"],
      // FIXME: unsafe-eval is necessary for wasm in Chrome for now until
      //   https://bugs.chromium.org/p/chromium/issues/detail?id=961485
      'script-src': ["'self'", 'blob:', "'unsafe-eval'"].concat(additionalScriptSrcs),
      'script-src-attr': ["'none'"],
      'style-src': ["'self'", 'https://fonts.googleapis.com', "'unsafe-inline'"],
      'upgrade-insecure-requests': [],
    };

    this.app.use(
      helmet({
        contentSecurityPolicy: {
          directives,
        },
        hsts: {
          maxAge: oneYearInSeconds,
          includeSubDomains: true,
          preload: true,
        },
      }),
    );
    this.app.use(
      cors({
        methods: ['GET'],
        allowedHeaders: ['Content-Type', 'Accept', 'User-Agent', 'Connection', 'If-None-Match'],
      }),
    );

    // support both JSON and x-url-encoded POST bodies
    this.app.use(
      bodyParser.json({
        limit: '10mb',
      }),
    );
    this.app.use(
      bodyParser.urlencoded({
        limit: '10mb',
        extended: false,
      }),
    );

    this.app.use(favicon(path.join(this.app.get('public'), 'favicon.ico')));

    authn(this.app);

    // authenticated:
    // /api is for API requests
    // all others should serve index.js if user is authorized
    this.app.use('/api', authz, apiRouter(this.app));

    const staticHandler = express.static('public', {
      // this doesn't seem to work on Google App Engine - always says
      // Tue, 01 Jan 1980 00:00:01 GMT, so disable it
      lastModified: false,
    });

    this.app.get(
      '/:username/:projectName',
      authz,
      async (req: Request, res: Response, next: NextFunction): Promise<void> => {
        const email = req.session.passport.user.email as string;
        const user = (req.user as any) as UserPb | undefined;
        if (!user) {
          logger.warn(`user not found for '${email}', but passed authz?`);
          res.status(500).json({});
          return;
        }
        // TODO
        if (user.getId() !== req.params.username) {
          res.status(401).json({});
          return;
        }

        if (
          req.path !== `/${req.params.username}/${req.params.projectName}` &&
          req.path !== `/${req.params.username}/${req.params.projectName}/`
        ) {
          res.status(404).json({});
          return;
        }

        const projectName: string = req.params.projectName;
        const projectId = `${user.getId()}/${projectName}`;
        const projectModel = await this.app.db.project.findOne(projectId);
        if (!projectModel || !projectModel.getFileId()) {
          res.status(404).json({});
          return;
        }

        req.url = '/index.html';

        // eslint-disable-next-line @typescript-eslint/await-thenable
        await next();
      },
      staticHandler,
    );

    // Configure a middleware for 404s and the error handler
    // this.app.use(express.notFound());
    // this.app.use(express.errorHandler({ logger }));

    // unauthenticated:
    // /static for CSS, JS, etc
    // /, /login serve index.js
    this.app.use(staticHandler);

    if (!this.app.db) {
      throw new Error('expected DB to be initialized');
    }
  }
}
