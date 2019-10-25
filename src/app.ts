// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as path from 'path';

import * as bodyParser from 'body-parser';
import * as cookieParser from 'cookie-parser';
import * as cors from 'cors';
import * as express from 'express';
import { Application, NextFunction, Request, Response } from 'express';
import * as helmet from 'helmet';
import * as favicon from 'serve-favicon';
import { seshcookie } from 'seshcookie';
import * as logger from 'winston';

import { apiRouter } from './api';
import { defined } from './app/common';
import { authn } from './authn';
import { authz } from './authz';
import { Project } from './models/project';
import { User } from './models/user';
import { mongoose } from './mongoose';
import { redirectToHttps } from './redirect-to-https';
import { requestLogger } from './request-logger';

export class App {
  app: Application;

  constructor() {
    this.app = express();

    this.setup();
  }

  private loadConfig(): void {
    const setConfig = async (filename: string) => {
      const contents = fs.readFileSync(filename).toString();
      const config = JSON.parse(contents);
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
        let obj: any = this.app.get(component);
        while (obj && path.length > 1) {
          component = defined(path[0]);
          path = path.slice(1);
          obj = obj[component];
        }
        if (obj) {
          obj[defined(path[0])] = value;
        }
      }
    }
  }

  private setup(): void {
    const { combine, timestamp, json } = logger.format;
    logger.configure({
      format: combine(timestamp(), json()),
      transports: [new logger.transports.Console({ level: 'debug' })],
    });

    const oneYearInSeconds = 365 * 24 * 60 * 60;

    this.loadConfig();

    this.app.use(requestLogger);
    this.app.use(cookieParser());
    this.app.use(seshcookie(this.app.get('authentication').seshcookie));
    this.app.use(
      helmet({
        hsts: {
          maxAge: oneYearInSeconds,
          includeSubDomains: true,
          preload: true,
        } as any, // FIXME: this avoids a hsts runtime deprecation warning
      }),
    );
    this.app.use(redirectToHttps);
    this.app.use(
      cors({
        methods: ['GET'],
        allowedHeaders: ['Content-Type', 'Accept', 'User-Agent', 'Connection', 'If-None-Match'],
      }),
    );

    // support both JSON and x-url-encoded POST bodies
    this.app.use(bodyParser.json());
    this.app.use(bodyParser.urlencoded({ extended: false }));

    this.app.use(favicon(path.join(this.app.get('public'), 'favicon.ico')));

    mongoose(this.app);
    authn(this.app);

    // authenticated:
    // /api is for API requests
    // all others should serve index.js if user is authorized
    this.app.use('/api', authz, apiRouter(this.app));

    this.app.get(
      '/:username/:projectName',
      authz,
      async (req: Request, res: Response, next: NextFunction): Promise<void> => {
        const email = req.session.passport.user.email;
        const user = await User.findOne({ email }).exec();
        if (!user) {
          logger.warn(`user not found for '${email}', but passed authz?`);
          res.status(500).json({});
          return;
        }
        // TODO
        if (user.username !== req.params.username) {
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
        const projectModel = await Project.findOne({ owner: user._id, name: projectName }).exec();
        if (!projectModel || !projectModel.fileId) {
          res.status(404).json({});
          return;
        }

        req.url = '/';

        await next();
      },
      express.static('public'),
    );

    // Configure a middleware for 404s and the error handler
    // this.app.use(express.notFound());
    // this.app.use(express.errorHandler({ logger }));

    // unauthenticated:
    // /static for CSS, JS, etc
    // /, /login serve index.js
    this.app.use(express.static('public'));
  }
}
