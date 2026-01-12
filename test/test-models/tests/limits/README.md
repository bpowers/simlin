Test Limits
===========

This model tests the ability to parse limits on variables as part of the units string.

```
Limited Flow=
	1
	~	Widgets/Month [-10,10,1]
	~	The flow is limited to between negative 10 and positive 10, and has \
		increments of 1
	|

Limited Stock= INTEG (
	Limited Flow,
		1)
	~	Widgets [0,100]
	~	The value of the stock is limited to between 0 and 100.
	|
```



![Vensim screenshot](vensim_screenshot.png)


Contributions
-------------

| Component                         | Author          | Contact                    | Date    | Software Version        |
|:--------------------------------- |:--------------- |:-------------------------- |:------- |:----------------------- |
| test_limits.mdl                   | James Houghton  | james.p.houghton@gmail.com | 4/05/16 | Vensim DSS 6.3E for Mac  |
| output.csv                        | James Houghton  | james.p.houghton@gmail.com | 8/30/15 | Vensim DSS 6.3E for Mac  |
