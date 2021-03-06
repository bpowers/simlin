convert [Vensim](https://vensim.com/vensim-software/) mdl files to [XMILE](http://docs.oasis-open.org/xmile/xmile/v1.0/cos01/xmile-v1.0-cos01.html#_Toc426543526)
=============================================

This is Bob Eberlein's [xmutil](https://github.com/bobeberlein/xmutil)
project compiled to WebAssembly and wrapped in the simplest possible
TypeScript wrapper (and also usable from plain JavaScript).

It is usable both the browser -- it should be easily adaptable to node as well (patches welcome!).

Usage
-----

```js
import { convertMdlToXmile } from '@system-dynamics/xmutil';

const args = process.argv.slice(2);
const mdlFile = fs.readFileSync(args[0], 'utf-8');

let xmile = await convertMdlToXmile(mdlFile, false);
console.log(xmile);
```

License
-------

@system-dynamics/xmutil is offered under the MIT license for consistency with the C++ xmutil.
