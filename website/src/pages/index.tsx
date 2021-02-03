import React from 'react';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import useBaseUrl from '@docusaurus/useBaseUrl';
import styles from './styles.module.css';

const features = [
  {
    title: 'Easy to Use',
    imageUrl: 'img/undraw_docusaurus_mountain.svg',
    description: (
      <>
        Simlin was designed from the ground up to be easily to use for leaders, managers, and developers.  Models are created in a simple visual language that can be picked up in minutes, yet is rich enough to describe domains from the carbon cycle to business dynamics.
      </>
    ),
  },
  {
    title: 'Easy to Share',
    imageUrl: 'img/undraw_docusaurus_tree.svg',
    description: (
      <>
        Developing the right strategy is the first step; convincing others it is the right strategy is the next.  Simlin makes it easy to share models on the web, embed them in blog posts, and print them out.
      </>
    ),
  },
  {
    title: 'Easy to Go Deep',
    imageUrl: 'img/undraw_docusaurus_react.svg',
    description: (
      <>
        Simlin is designed to be simple, but interops with tools like <a href="https://pysd.readthedocs.io/en/master/index.html">PySD</a> if you have more advanced needs, like fitting model parameters, running a sensitivity analysis, or working with geographic data.
      </>
    ),
  },
];

function Feature({imageUrl, title, description}) {
  const imgUrl = useBaseUrl(imageUrl);
  return (
    <div className={clsx('col col--4', styles.feature)}>
      {imgUrl && (
        <div className="text--center">
          <img className={styles.featureImage} src={imgUrl} alt={title} />
        </div>
      )}
      <h3>{title}</h3>
      <p>{description}</p>
    </div>
  );
}

function Home() {
  const context = useDocusaurusContext();
  const {siteConfig = {}} = context;
  return (
    <Layout
      title={`Hello from ${siteConfig.title}`}
      description="Description will go into a meta tag in <head />">
      <header className={clsx('hero hero--primary', styles.heroBanner)}>
        <div className="container">
          <h1 className="hero__title">{siteConfig.title}</h1>
          <p className="hero__subtitle">{siteConfig.tagline}</p>
          <p>
            Simlin is a tool for simulation modeling, leveling up your ability to learn.
            With Simlin you can iterate on policy and strategy much faster (and with fewer costs and consequences) than you can in the real world.
          </p>
          <div className={styles.buttons}>
            <Link
              className={clsx(
                'button button--outline button--lg',
                styles.getStarted,
              )}
              to={'https://app.simlin.com'}>
              Get Started
            </Link>
          </div>
        </div>
      </header>
      <main>
        {features && features.length > 0 && (
          <section className={styles.features}>
            <div className="container">
              <div className="row">
                {features.map((props, idx) => (
                  <Feature key={idx} {...props} />
                ))}
              </div>
            </div>
          </section>
        )}
      </main>
    </Layout>
  );
}

export default Home;
