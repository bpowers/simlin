---
id: simlin-app
title: The Simlin App
sidebar_label: The Simlin App
slug: /
---

import { open } from '@system-dynamics/engine';
import { toUint8Array } from 'js-base64';

export const logisticGrowthProject = 'Cg9sb2dpc3RpYy1ncm93dGgSFBEAAAAAAABZQBoJCQAAAAAAAPA/GrYGCgRtYWluGiUKIwoKcG9wdWxhdGlvbioObmV0X2JpcnRoX3JhdGVCBQoDCgE1GjsSOQoObmV0X2JpcnRoX3JhdGVCJwolCiNmcmFjdGlvbmFsX2dyb3d0aF9yYXRlICogcG9wdWxhdGlvbhogGh4KE21heGltdW1fZ3Jvd3RoX3JhdGUyBwoFCgMuMTIaHxodChFjYXJyeWluZ19jYXBhY2l0eTIICgYKBDEwMDAaXhpcChZmcmFjdGlvbmFsX2dyb3d0aF9yYXRlMkIKQAo+bWF4aW11bV9ncm93dGhfcmF0ZSAqICgxIC0gZnJhY3Rpb25fb2ZfY2FycnlpbmdfY2FwYWNpdHlfdXNlZCkaSBpGCiJmcmFjdGlvbl9vZl9jYXJyeWluZ19jYXBhY2l0eV91c2VkMiAKHgoccG9wdWxhdGlvbi9jYXJyeWluZ19jYXBhY2l0eSLcAxoiEiAKClBvcHVsYXRpb24QARkAAABAM9OCQCEAAADgzDxwQBpSGlAKDm5ldCBiaXJ0aCByYXRlEAIZqvbqaC4ygEAhAAAAwJkpcEAyFAkAAADAzIx8QBEAAADAmSlwQBgNMhQJAAAAQDMfgkARAAAAwJkpcEAYARotCisKE21heGltdW0KZ3Jvd3RoIHJhdGUQAxnNzMxMM/N8QCEAAABACgd5QCgBGisKKQoRY2FycnlpbmcgY2FwYWNpdHkQBBmZmZlZuF6EQCFnZmbm1vN4QCgEGjEKLwoXZnJhY3Rpb25hbCAKZ3Jvd3RoIHJhdGUQBRkAAAAAACB/QCEAAABAM4N1QCgBGjwKOgoiZnJhY3Rpb24gb2YgY2FycnlpbmcKY2FwYWNpdHkgdXNlZBAGGQAAAIBmFoNAIQAAAKCZiXVAKAQaESIPCAcQBBgGIWZANqZnymxAGhEiDwgIEAEYBiEgMB5yvNFIQBoRIg8ICRAGGAUhjENNVBh1YEAaESIPCAoQAxgFIbDL4YNiz3JAGhEiDwgLEAUYAiHS1VhKsiVuQBoRIg8IDBABGAIh4KA0JrmkWUAaGDoWCA0QAhkAAADAzIx8QCEAAADAmSlwQCIAKQAAAAAAAPA/';
export const serializedProject = toUint8Array(logisticGrowthProject);
export const engine = await open(serializedProject);

console.log(engine);

Simlin enables you to build [system dynamics](https://systemdynamics.org/what-is-system-dynamics/) models.
System dynamics models are explicit, formal representations of our assumptions about how a system works.

[Stella](https://www.iseesystems.com/store/products/stella-professional.aspx) by [isee systems](https://www.iseesystems.com/) and [Vensim](https://vensim.com/) by [Ventana Systems](https://www.ventanasystems.com/) are the existing state of the art tools used by practitioners for modeling.
Simlin differs from these tools in three big ways:

1. Unrestricted creation. [Stella Online](https://www.iseesystems.com/store/products/stella-online.aspx) limits free users to models with thirteen variables, and [Vensim Personal Learning Edition](https://vensim.com/vensim-personal-learning-edition/) requires a license for non-educational use.
   Simlin allows you to build, import, and export models without restrictions.  In the future we may have paid tiers around collaboration or advanced features, but we strongly believe there shouldn't be a barrier to getting started with and practicing system dynamics modeling.
2. Web-based.  Simlin doesn't require downloading, installing, and updating desktop-based software.  It is designed from the ground up to run well in your browser.
3. Fewer features.  Simlin focuses on the modeling process for small to mid-sized models, and doesn't have the advanced features for calibration, optimization, and sensitivity analysis that Vensim or Stella does.  Simlin allows you to export your model in standard [XMILE](http://docs.oasis-open.org/xmile/xmile/v1.0/cos01/xmile-v1.0-cos01.html) format, so you can always start models in Simlin and transfer them to Stella or the latest Vensim for advanced analysis later.  

Features:
* Create models with stocks, flows, and auxiliary variables.
* Import models from Vensim (`*.mdl`) and Stella (`*.stmx`), and export a model as XMILE at any time.
* Basic unit checking of equations.

In Progress:
* Unit checking of smooth and delay builtins and inference of not-yet-specified units.
* Support for modules and arrayed variables.  Simlin can correctly simulate and show results for imported models with these features, but we haven't yet implemented support for creating and editing them.

Planned:
* Reference modes.  Models are representations of systems in the real world: to gain confidence in the model, we need to easily be able to compare its output with historical data and metrics.
* Run management.  As you edit your module, its invaluable to be able to compare the current results to results from a previous version.
* Graphs and results exploration.
* Compare model versions and restore old versions.
* Collaboration.  It should be as easy to have multiple people working on a model as it is to edit a google doc. 


<!--
* Initial screen shows your models
* Click to open

* how to click, move the canvas around, etc 
-->

