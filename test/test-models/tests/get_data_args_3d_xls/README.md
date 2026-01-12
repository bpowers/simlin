Test get data args 3d xls
============================

This model tests the data imported using GET XLS  DATA with arguments (XLS and DIRECT are identically implemented) for subscripted 3D matrix from an Excel file. Both of them are identically initialized in PySD. All the possible combinations from 0D to 3D are tested in the `unit_test_external.py`. This test aims to ensure the performance of the builder in the creation of the Python object, using 3D data to make sure it works well when the variable is defined in both one and several groups in the mdl file. The DATA is passed with LOOK FORWARD/HOLD BACKWARD arguments to ensure that this argument is properly read. This requires black package, which is only available for Python 3.6+


Contributions
-------------

| Component                   | Author          | Contact                         | Date    | Software Version                                      |
|:--------------------------- |:--------------- |:------------------------------- |:-------- |:---------------------------------------------------- |
| `get_data_args_3d_xls.mdl`  | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `input.xls`                 | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `output.tab `               | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
