import {test as brittleTest, hook, solo as brittleSolo} from 'brittle';

const solo = (label, callback) => {
  brittleSolo(label, async (t) => {
    try{
      await callback(t)
    } catch (error) {
      t.fail(`FAIL: ${label}`, error?.message || error);
    }
  })
}

const test = (label, callback) => {
  brittleTest(label, async (t) => {
    try{
      await callback(t)
    } catch (error) {
      console.log(error?.message)
      t.fail(`FAIL: ${label}`, error?.message || error);
    }
  })
}

export {
  test,
  hook,
  solo
}