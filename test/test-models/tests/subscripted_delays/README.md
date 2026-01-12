Test Subscripted Delays
=======================

This model tests basic delay functions with subscripted inputs and outputs, and subscripted delay parameters:

- Delay1, Delay3, DelayN
- Delay1I, Delay3I

It does not test infinite order delays, as these require a separate representation, 
despite being similar in concept, and not all tools will choose to support it.

It also does not test delays in which the order of the delay varies with the subscript. Such a case would be pathological.

![Vensim screenshot](vensim_screenshot.png)

The current test is shorter than the original, and a numerical error found in the original one has been reproduced in test delay numeric error.

Contributions
-------------

| Component                         | Author          | Contact                    | Date    | Software Version                                          |
|:--------------------------------- |:--------------- |:-------------------------- |:------- |:--------------------------------------------------------- |
| `test_subscripted_delays.mdl`     | James Houghton  | james.p.houghton@gmail.com      | 2/04/16  | Vensim DSS 6.3E for Mac                             |
| `output.tab`                      | James Houghton  | james.p.houghton@gmail.com      | 2/04/16  | Vensim DSS 6.3E for Mac                             |
| `test_subscripted_delays.mdl`     | Eneko Martin    | eneko.martin.martinez@gmail.com | 12/14/20 | Vensim DSS for Windows 7.3.4 single precision (x32) |
| `output.tab`                      | Eneko Martin    | eneko.martin.martinez@gmail.com | 12/14/20 | Vensim DSS for Windows 7.3.4 single precision (x32) |

