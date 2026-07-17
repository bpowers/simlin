// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { Helmet } from 'rspress/runtime';

import { PopulationModel } from './PopulationModel';

interface Feature {
  title: string;
  description: React.ReactNode;
}

const features: Feature[] = [
  {
    title: 'Easy to Use',
    description: (
      <>
        Simlin was designed from the ground up to be easily to use for leaders, managers, and developers. Models are
        created in a simple visual language that can be picked up in minutes, yet is rich enough to describe domains
        from the carbon cycle to business dynamics.
      </>
    ),
  },
  {
    title: 'Easy to Share',
    description: (
      <>
        Developing the right strategy is the first step; convincing others it is the right strategy is the next. Simlin
        makes it easy to share models on the web, embed them in blog posts, and print them out.
      </>
    ),
  },
  {
    title: 'Easy to Go Deep',
    description: (
      <>
        Simlin works seamlessly with open-source tools like{' '}
        <a href="https://pysd.readthedocs.io/en/master/index.html">PySD</a> and proprietary software like{' '}
        <a href="https://www.iseesystems.com/store/products/stella-architect.aspx">Stella</a> for more advanced tasks
        like fitting model parameters, running sensitivity analyses, or working with geographic data.
      </>
    ),
  },
];

// A from-scratch home layout mirroring the classic Docusaurus homepage this
// site previously shipped: red hero banner, prose introduction, the live
// population-model diagram, and a three-column feature summary.
export function HomePage(): React.ReactElement {
  return (
    <main>
      <Helmet>
        <title>Simlin system dynamics software | Simlin</title>
        <meta name="description" content="System dynamics modeling software" />
      </Helmet>
      <header className="simlin-hero">
        <div className="simlin-container simlin-hero-container">
          <h1 className="simlin-hero-title">Simlin</h1>
          <p className="simlin-hero-subtitle">Debug your intuition</p>
          <p>
            Simlin is a tool for simulation modeling, leveling up your ability to learn. With Simlin you can iterate on
            strategy and policy faster than you can in the real world, with fewer costs and the freedom to fail.
          </p>
          <div className="simlin-hero-buttons">
            <a className="simlin-get-started" href="https://app.simlin.com">
              Get Started
            </a>
          </div>
        </div>
      </header>
      <br />
      <br />
      <div className="simlin-container">
        We all have mental models of the world around us, and we use these mental models when we create and evaluate
        plans to achieve our goals. Simlin gives you the power to turn these implicit models explicit, so you can debug
        and improve them.
      </div>
      <br />
      <div className="simlin-container">
        Simlin is built around the <a href="https://en.wikipedia.org/wiki/System_dynamics">system dynamics</a>{' '}
        methodology, as introduced by the{' '}
        <a href="https://www.chelseagreen.com/product/thinking-in-systems/">Thinking in Systems</a> book (among others).
        You can learn more and connect with experts by engaging with the{' '}
        <a href="http://www.systemdynamics.org/">System Dynamics Society</a>. Models are built in a simple, general
        visual language:
      </div>
      <br />
      <div className="simlin-container simlin-diagram-container">
        <PopulationModel />
      </div>
      <br />
      <section className="simlin-features">
        <div className="simlin-container">
          <div className="simlin-features-row">
            {features.map((feature) => (
              <div className="simlin-feature" key={feature.title}>
                <h3>{feature.title}</h3>
                <p>{feature.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>
    </main>
  );
}
