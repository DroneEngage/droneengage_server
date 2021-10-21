
var Me = this;




exports.fn_getCSSTraffic = function fn_getCSSTraffic (p_getCSSTraffic)
{
    var TXCSS = "text-disabled";
       

	if (p_getCSSTraffic <= 1000)
    {
        TXCSS = "text-danger";
    }
    else if (p_getCSSTraffic <= 10000)
    {
        TXCSS = "text-warning";
    }
    else if (p_getCSSTraffic <= 100000)
    {
        TXCSS = "text-primary";
    }
    else if (p_getCSSTraffic<= 1000000)
    {
        TXCSS = "text-success";
    }

    return TXCSS;
}