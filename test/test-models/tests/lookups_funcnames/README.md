Test Lookup / Function confusion 
============

Lookups and functions have the same syntax, need to make sure they don't interfere with each others parsing.

For instance, a lookup that starts with `EXP` might accidentally get parsed as the exp function, and this wouldn't be caught by any of the later parsing options.

Alternately, we need to make sure that something that should be parsed as a function doesn't accidentally get parsed as a lookup, for the same reason.

![test_lookups Vensim screenshot](vensim_screenshot.png)


Contributions
-------------

| Component                      | Author          | Contact                    | Date    | Software Version        |
|:------------------------------ |:--------------- |:-------------------------- |:------- |:----------------------- |
| test_lookups_funcnames.mdl               | James Houghton  | james.p.houghton@gmail.com | 2/14/17 | Vensim DSS 6.3 for Mac  |
| output.tab                     | James Houghton  | james.p.houghton@gmail.com | 2/14/17 | Vensim DSS 6.3 for Mac  |