”
stdlibÂ·smth3<:
flow_1 (input - Stock_1)/(delay_time/3)"unit/time unitQ
O
stock_16IF (initial_value = NAN) THEN input ELSE initial_value"unit*flow_1Q
O
stock_26IF (initial_value = NAN) THEN input ELSE initial_value"unit*flow_2><
flow_2"(Stock_1 - Stock_2)/(delay_time/3)"unit/time unitP
N
output6IF (initial_value = NAN) THEN input ELSE initial_value"unit*flow_3=;
flow_3!(Stock_2 - Output)/(delay_time/3)"unit/time unit

delay_time1"	time unit
input0"unit
initial_valueNAN"unit