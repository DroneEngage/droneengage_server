
exports.fn_dumperror = function fn_dumperror(err) {
    console.log (err);
  }
  
  
  exports.dumperror2 = function dumperror2(err) {
    if (typeof err === 'object') {
      if (err.message) {
        console.log('\nMessage: ' + err.message)
      }
      if (err.stack) {
        console.log('\nStacktrace:')
        console.log('====================')
        console.log(err.stack);
      }
    } else {
      console.log('dumpError :: argument is not an object');
    }
  }


  exports.fn_dumpdebug = function dumperror2(err) {
    if (err == null) return;
    if (typeof err === 'object') {
      if (err.message) {
        console.log('\nMessage: ' + err.message)
      }
      if (err.stack) {
        console.log('\nStacktrace:')
        console.log('====================')
        console.log(err.stack);
      }
    } 
    else if (typeof err === 'string') 
    {
      console.log(err);
    }
    else 
    {
      console.log('dumpError :: argument is not an object');
    }
  }
  