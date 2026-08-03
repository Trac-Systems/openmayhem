import test from 'brittle';
import { ACK_OPERATION_TYPE, Updater } from '../../src/tasks/updater.js';

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));

test('updater acknowledges unsigned indexer entries with signed no-op values', async (t) => {
    const appended = [];

    const updater = new Updater({
        base: {
            isIndexer: true,
            view: {
                core: {
                    length: 1,
                    signedLength: 0
                }
            },
            async ack() {
                throw new Error('autobase null ack path must not be used');
            },
            async append(value) {
                appended.push(value);
            }
        }
    }, { updaterIntervalMs: 5 });

    await updater.start();
    await sleep(25);
    await updater.stop();

    t.ok(appended.length > 0, 'unsigned entries are acknowledged');
    t.ok(appended.every(value => value !== null), 'raw null append is not used for acknowledgements');
    t.is(appended[0].type, ACK_OPERATION_TYPE, 'acknowledgement uses the reserved no-op operation');
});
