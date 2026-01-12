Tests nested sets of arguments
============
For instance, when we have functions with multiple arguments, and those arguments themselves are expressions or functions with their own arguments, do the right functions get the right arguments?

For example:
```
SMOOTH N(DELAY3( Time , MODULO( constant , 3 ) ), constant , 1 , 2 )
```

In response to: https://github.com/SDXorg/pysd/issues/104

Contributions
-------------

| Component             | Author          | Contact                    | Date    | Software Version          |
|:--------------------- |:--------------- |:-------------------------- |:------- |:------------------------- |
| test_arguments.mdl    | James Houghton    | james.p.houghton@gmail.com      | 10/7/16 | Vensim for Mac 6.4b     |
| output.tab            | James Houghton    | james.p.houghton@gmail.com      | 10/7/16 | Vensim for Mac 6.4b    |
