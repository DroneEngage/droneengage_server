
exports.fn_dumperror = function fn_dumperror(err) {
  console.log (err);
}


exports.fn_dumperror2 = function fn_dumperror2(err) {
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
