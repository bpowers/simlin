import React from 'react';
import { renderToString } from 'react-dom/server';
import clsx from 'clsx';
import Layout from '@theme/Layout';
import useThemeContext from '@theme/hooks/useThemeContext';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import useBaseUrl from '@docusaurus/useBaseUrl';
import { toUint8Array } from 'js-base64';
import { Set } from 'immutable';
import { createMuiTheme } from '@material-ui/core/styles';
import { ServerStyleSheets, ThemeProvider } from "@material-ui/styles";

import { defined } from '@system-dynamics/core/common';
import { UID, ViewElement, Project } from '@system-dynamics/core/datamodel';
import { Point } from '@system-dynamics/diagram/drawing/common';
import { Canvas } from '@system-dynamics/diagram/drawing/Canvas';

import styles from './styles.module.css';

const reliabilityProject = 'Eh4RAAAAAAAANEAaCwkAAAAAAAAQQBABMgZNb250aHManBwKBG1haW4aSgpICglpbmNpZGVudHMiCGluY2lkZW50KhJjcmVhdGluZ19pbmNpZGVudHMyFG1pdGlnYXRpbmdfaW5jaWRlbnRzOAFCBQoDCgE5Gk0KSwoJbWl0aWdhdGVkIghpbmNpZGVudCoUbWl0aWdhdGluZ19pbmNpZGVudHMyFXJlbWVkaWF0aW5nX2luY2lkZW50czgBQgUKAwoBMhqBARJ/ChJjcmVhdGluZ19pbmNpZGVudHMiD2luY2lkZW50L01vbnRoczgBQlYKVApSY2hhbmdlcyptYXJnaW5hbF9pbmNpZGVudHNfcGVyX2NoYW5nZSogZWZmZWN0X29mX3JlbWVkaWF0aW9uc19vbl9pbmNpZGVudF9jcmVhdGlvbhpsEmoKFG1pdGlnYXRpbmdfaW5jaWRlbnRzIg9pbmNpZGVudC9Nb250aHM4AUI/Cj0KO2RldmVsb3BlcnNfcmVxdWlyZWRfZm9yX2luY2lkZW50X21pdGlnYXRpb24qbWl0aWdhdGlvbl9yYXRlGisKKQoKZGV2ZWxvcGVycyIJZGV2ZWxvcGVyKgZoaXJpbmc4AUIGCgQKAjEwGiUSIwoGaGlyaW5nIhBkZXZlbG9wZXIvTW9udGhzOAFCBQoDCgE1GnASbgoHY2hhbmdlcyISbGluZXNvZmNvZGUvTW9udGhzOAFCTQpLCklhdmFpbGFibGVfZGV2ZWxvcGVyc19mb3JfcHJvZHVjdF93b3JrKm1hcmdpbmFsX3Byb2R1Y3Rpdml0eV9wZXJfZGV2ZWxvcGVyGjwKOgoZcHJvZHVjdF9jb21wcmVoZW5zaXZlbmVzcyILbGluZXNvZmNvZGUqB2NoYW5nZXM4AUIFCgMKATAaSxpJCiNtYXJnaW5hbF9wcm9kdWN0aXZpdHlfcGVyX2RldmVsb3BlciIbbGluZXNvZmNvZGUvZGV2ZWxvcGVyL21vbnRoMgUKAwoBMhqnARqkAQolYXZhaWxhYmxlX2RldmVsb3BlcnNfZm9yX3Byb2R1Y3Rfd29yayIJZGV2ZWxvcGVyMnAKbgpsTUFYKDAsICBEZXZlbG9wZXJzLWRldmVsb3BlcnNfcmVxdWlyZWRfZm9yX2luY2lkZW50X21pdGlnYXRpb24tZGV2ZWxvcGVyc19yZXF1aXJlZF9mb3JfaW5jaWRlbnRfcmVtZWRpYXRpb24pGkAaPgodbWFyZ2luYWxfaW5jaWRlbnRzX3Blcl9jaGFuZ2UiFGluY2lkZW50L2xpbmVzb2Zjb2RlMgcKBQoDMC4yGjgKNgoKcmVtZWRpYXRlZCIIaW5jaWRlbnQqFXJlbWVkaWF0aW5nX2luY2lkZW50czgBQgUKAwoBMRpvEm0KFXJlbWVkaWF0aW5nX2luY2lkZW50cyIPaW5jaWRlbnQvTW9udGhzOAFCQQo/Cj1kZXZlbG9wZXJzX3JlcXVpcmVkX2Zvcl9pbmNpZGVudF9yZW1lZGlhdGlvbipyZW1lZGlhdGlvbl9yYXRlGjUaMwogcmVtZWRpYXRpb25fZWZmZWN0aXZlbmVzc19mYWN0b3IiCGluY2lkZW50MgUKAwoBMRpzGnEKK2VmZmVjdF9vZl9yZW1lZGlhdGlvbnNfb25faW5jaWRlbnRfY3JlYXRpb24iBGRtbmwyPAo6CjhTQUZFRElWKCByZW1lZGlhdGlvbl9lZmZlY3RpdmVuZXNzX2ZhY3RvciAsIFJlbWVkaWF0ZWQgKRpPGk0KMGF2ZXJhZ2VfZWZmb3J0X3JlcXVpcmVkX3RvX3JlbWVkaWF0ZV9hbl9pbmNpZGVudCISZGV2ZWxvcGVyL2luY2lkZW50MgUKAwoBMRo2GjQKD21pdGlnYXRpb25fcmF0ZSIYaW5jaWRlbnQvZGV2ZWxvcGVyL21vbnRoMgcKBQoDMC4xGnoaeAorZGV2ZWxvcGVyc19yZXF1aXJlZF9mb3JfaW5jaWRlbnRfbWl0aWdhdGlvbiIJZGV2ZWxvcGVyMj4KPAo6SW5jaWRlbnRzKmF2ZXJhZ2VfZWZmb3J0X3JlcXVpcmVkX3RvX3JlbWVkaWF0ZV9hbl9pbmNpZGVudBpOGkwKL2F2ZXJhZ2VfZWZmb3J0X3JlcXVpcmVkX3RvX21pdGlnYXRlX2FuX2luY2lkZW50IhJkZXZlbG9wZXIvaW5jaWRlbnQyBQoDCgE0GnoaeAosZGV2ZWxvcGVyc19yZXF1aXJlZF9mb3JfaW5jaWRlbnRfcmVtZWRpYXRpb24iCWRldmVsb3BlcjI9CjsKOU1pdGlnYXRlZCphdmVyYWdlX2VmZm9ydF9yZXF1aXJlZF90b19t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';

function Diagram(props) {
  const canUseDOM = !!(
    typeof window !== 'undefined' &&
    window.document &&
    window.document.createElement
  );
  const { isDarkTheme } = useThemeContext();
  const theme = React.useMemo(
    () =>
      createMuiTheme({
        palette: {
          mode: isDarkTheme ? 'dark' : 'light',
          common: {
            white: isDarkTheme ? '#222222' : '#ffffff',
            black: isDarkTheme ? '#bbbbbb' : '#000000',
          },
        },
      }),
    [isDarkTheme],
  );

  const project = Project.deserializeBinary(toUint8Array(props.projectPbBase64));
  const model = defined(project.models.get('main'));

  const renameVariable = (_oldName: string, _newName: string): void => {};
  const onSelection = (_selected: Set<UID>): void => {};
  const moveSelection = (_position: Point): void => {};
  const moveFlow = (_element: ViewElement, _target: number, _position: Point): void => {};
  const moveLabel = (_uid: UID, _side: 'top' | 'left' | 'bottom' | 'right'): void => {};
  const attachLink = (_element: ViewElement, _to: string): void => {};
  const createCb = (_element: ViewElement): void => {};
  const nullCb = (): void => {};

  const canvasElement = (
    <Canvas
      embedded={true}
      project={project}
      model={model}
      view={defined(model.views.get(0))}
      version={1}
      selectedTool={undefined}
      selection={Set()}
      onRenameVariable={renameVariable}
      onSetSelection={onSelection}
      onMoveSelection={moveSelection}
      onMoveFlow={moveFlow}
      onMoveLabel={moveLabel}
      onAttachLink={attachLink}
      onCreateVariable={createCb}
      onClearSelectedTool={nullCb}
      onDeleteSelection={nullCb}
      onShowVariableDetails={nullCb}
      onViewBoxChange={nullCb}
    />
  );

  const themedCanvas = <ThemeProvider theme={theme}>{canvasElement}</ThemeProvider>;

  if (canUseDOM) {
    return themedCanvas;
  } else {
    const sheets = new ServerStyleSheets();
    renderToString(sheets.collect(themedCanvas));
    return <>
      {sheets.getStyleElement()}
      {themedCanvas}
    </>;
  }
}

const features = [
  {
    title: 'Easy to Use',
    imageUrl: 'img/undraw_docusaurus_mountain.svg',
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
    imageUrl: 'img/undraw_docusaurus_tree.svg',
    description: (
      <>
        Developing the right strategy is the first step; convincing others it is the right strategy is the next. Simlin
        makes it easy to share models on the web, embed them in blog posts, and print them out.
      </>
    ),
  },
  {
    title: 'Easy to Go Deep',
    imageUrl: 'img/undraw_docusaurus_react.svg',
    description: (
      <>
        Simlin works seamlessly with open-source tools like{' '}
        <a href="https://pysd.readthedocs.io/en/master/index.html">PySD</a>{' '}
        and proprietary software like{' '}
        <a href="https://www.iseesystems.com/store/products/stella-architect.aspx">Stella</a>{' '}
        for more advanced tasks like fitting model parameters,
        running sensitivity analyses, or working with geographic data.
      </>
    ),
  },
];

function Feature({ imageUrl, title, description }) {
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
  const { siteConfig = {} } = context;

  return (
    <Layout title={`Hello from ${siteConfig.title}`} description="Description will go into a meta tag in <head />">
      <header className={clsx('hero hero--primary', styles.heroBanner)}>
        <div className="container">
          <h1 className="hero__title">{siteConfig.title}</h1>
          <p className="hero__subtitle">{siteConfig.tagline}</p>
          <p>
            Simlin is a tool for simulation modeling, leveling up your ability to learn. With Simlin you can iterate on
            policy and strategy faster than you can in the real world, with fewer costs and the freedom to fail.
          </p>
          <div className={styles.buttons}>
            <Link
              className={clsx('button button--outline button--lg', styles.getStarted)}
              to={'https://app.simlin.com'}
            >
              Get Started
            </Link>
          </div>
        </div>
      </header>
      <main>
        <br />
        <br />
        <div className="container">
          {/* what you get: simulation modeling to define the _Structure_ of the model you have that generates the behavior you are interested in affecting */}
        </div>
        <div className="container">
          {/* how is this different from machine learning? focus on _causation_ rather than correlation; different domains where $x, $y, $z are best suited for SD */}
        </div>
        <div className="container">
          {/* simlin is built on the back of a 60+ year old field of research started at MIT */}
        </div>
        <div className="container">
          <Diagram projectPbBase64={ reliabilityProject } />
        </div>
        <br />
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
