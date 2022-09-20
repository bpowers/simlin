// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::str;
use std::sync::Mutex;

use lazy_static::lazy_static;

extern "C" {
    fn xmutil_convert_mdl_to_xmile(
        mdl_source: *const u8,
        mdl_source_len: u32,
        file_name: *const u8,
        is_compact: bool,
        is_long_name: bool,
        is_as_sectors: bool,
    ) -> *mut c_char;
    fn xmutil_get_log() -> *const c_char;
    fn xmutil_clear_log();
}

// xmutil isn't thread-safe, so we need to synchronize calls into it.
lazy_static! {
    static ref LOCK: Mutex<()> = Mutex::new(());
}

pub fn convert_vensim_mdl(mdl_source: &str, is_compact: bool) -> (Option<String>, Option<String>) {
    // always grab the lock guard before calling in to _convert_mdl_to_xmile
    let _guard = LOCK.lock().unwrap();

    let str_ptr = mdl_source.as_ptr();
    let str_len = mdl_source.len() as u32;

    unsafe {
        // ensure the log starts empty
        xmutil_clear_log();

        let result_buf = xmutil_convert_mdl_to_xmile(
            str_ptr,
            str_len,
            std::ptr::null(),
            is_compact,
            false,
            false,
        );

        let log_buf = xmutil_get_log();
        let log = if !log_buf.is_null() {
            // a reference to non-owned data
            let c_str: &CStr = CStr::from_ptr(log_buf);
            let str_slice: &str = c_str.to_str().unwrap();
            if str_slice.is_empty() {
                None
            } else {
                Some(str_slice.to_owned())
            }
        } else {
            None
        };

        let model = if !result_buf.is_null() {
            // take ownership of the returned string
            CString::from_raw(result_buf)
                .into_string()
                .ok()
                .filter(|s| !s.is_empty())
        } else {
            None
        };

        (model, log)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let mdl_source = "{UTF-8}
Inflow=
	IF THEN ELSE(Time = INITIAL TIME , 10 , 3 )
	~
	~		|

Outflow 1=
	Stock/TIME STEP
	~
	~		|

Outflow 2=
	IF THEN ELSE( Time = FINAL TIME , 2 , 0 )
	~
	~		|

Stock= INTEG (
	Inflow-Outflow 1-Outflow 2,
		0)
	~
	~		|

********************************************************
	.Control
********************************************************~
		Simulation Control Parameters
	|

FINAL TIME  = 10
	~	Month
	~	The final time for the simulation.
	|

INITIAL TIME  = 0
	~	Month
	~	The initial time for the simulation.
	|

SAVEPER  =
        TIME STEP
	~	Month [0,?]
	~	The frequency with which output is stored.
	|

TIME STEP  = 1
	~	Month [0,?]
	~	The time step for the simulation.
	|

\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|72,72,100,0
10,1,Stock,351,249,40,20,3,3,0,0,0,0,0,0
12,2,48,513,251,10,8,0,3,0,0,-1,0,0,0
1,4,6,2,4,0,0,22,0,0,0,-1--1--1,,1|(478,251)|
1,5,6,1,100,0,0,22,0,0,0,-1--1--1,,1|(416,251)|
11,6,0,447,251,6,8,34,3,0,0,1,0,0,0
10,7,Outflow 1,447,267,27,8,40,3,0,0,-1,0,0,0
1,8,1,7,1,0,0,0,0,128,0,-1--1--1,,1|(394,294)|
10,9,TIME STEP,485,317,39,8,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|0||128-128-128
1,10,9,7,0,0,0,0,0,128,0,-1--1--1,,1|(470,297)|
12,11,48,191,244,10,8,0,3,0,0,-1,0,0,0
1,13,15,1,4,0,0,22,0,0,0,-1--1--1,,1|(286,244)|
1,14,15,11,100,0,0,22,0,0,0,-1--1--1,,1|(225,244)|
11,15,0,256,244,6,8,34,3,0,0,1,0,0,0
10,16,Inflow,256,260,18,8,40,3,0,0,-1,0,0,0
10,17,INITIAL TIME,190,309,47,8,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|0||128-128-128
1,18,17,16,0,0,0,0,0,128,0,-1--1--1,,1|(216,288)|
10,19,Time,292,310,21,8,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|0||128-128-128
1,20,19,16,0,0,0,0,0,128,0,-1--1--1,,1|(278,290)|
12,21,48,355,133,10,8,0,3,0,0,-1,0,0,0
1,23,25,21,4,0,0,22,0,0,0,-1--1--1,,1|(356,159)|
1,24,25,1,100,0,0,22,0,0,0,-1--1--1,,1|(356,209)|
11,25,0,356,183,8,6,33,3,0,0,4,0,0,0
10,26,Outflow 2,391,183,27,8,40,3,0,0,-1,0,0,0
10,27,Time,428,131,21,8,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|0||128-128-128
10,28,FINAL TIME,494,188,43,8,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|0||128-128-128
1,29,28,26,0,0,0,0,0,128,0,-1--1--1,,1|(441,185)|
1,30,27,26,0,0,0,0,0,128,0,-1--1--1,,1|(413,151)|
///---\\\\\\
:L<%^E!@
1:Current.vdf
9:Current
22:$,Dollar,Dollars,$s
22:Day,Days
22:Hour,Hours
22:Month,Months
22:Person,People,Persons
22:Unit,Units
22:Week,Weeks
22:Year,Years
15:0,0,0,0,0,0
19:100,0
27:2,
34:0,
4:Time
5:Stock
35:Date
36:YYYY-MM-DD
37:2000
38:1
39:1
40:2
41:0
42:1
24:0
25:10
26:10
";

        let (actual, logs) = crate::convert_vensim_mdl(mdl_source, false);
        let actual = actual.unwrap();
        assert!(actual.starts_with("<xmile "));
        assert!(actual.ends_with("</xmile>\n"));
        assert!(logs.is_none());
    }

    #[test]
    fn failure_is_none() {
        let (xmile, logs) = crate::convert_vensim_mdl(":ohno:", true);

        assert!(xmile.is_none());
        assert!(logs.is_some());
    }
}
