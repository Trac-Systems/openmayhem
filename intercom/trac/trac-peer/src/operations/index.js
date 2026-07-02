import { TxOperation, TxCheck } from './tx/index.js';
import { MsgOperation, MsgCheck } from './msg/index.js';
import { FeatureOperation, FeatureCheck } from './feature/index.js';
import { AddIndexerOperation, AddIndexerCheck } from './addIndexer/index.js';
import { AddWriterOperation, AddWriterCheck } from './addWriter/index.js';
import { RemoveWriterOperation, RemoveWriterCheck } from './removeWriter/index.js';
import { SetChatStatusOperation, SetChatStatusCheck } from './setChatStatus/index.js';
import { SetAutoAddWritersOperation, SetAutoAddWritersCheck } from './setAutoAddWriters/index.js';
import { AutoAddWritersOperation, AutoAddWritersCheck } from './autoAddWriter/index.js';
import { AddAdminOperation, AddAdminCheck } from './addAdmin/index.js';
import { UpdateAdminOperation, UpdateAdminCheck } from './updateAdmin/index.js';
import { SetNickOperation, SetNickCheck } from './setNick/index.js';
import { MuteStatusOperation, MuteStatusCheck } from './muteStatus/index.js';
import { DeleteMessageOperation, DeleteMessageCheck } from './deleteMessage/index.js';
import { UnpinMessageOperation, UnpinMessageCheck } from './unpinMessage/index.js';
import { PinMessageOperation, PinMessageCheck } from './pinMessage/index.js';
import { SetModOperation, SetModCheck } from './setMod/index.js';
import { SetWhitelistStatusOperation, SetWhitelistStatusCheck } from './setWhitelistStatus/index.js';
import { EnableWhitelistOperation, EnableWhitelistCheck } from './enableWhitelist/index.js';
import { EnableTransactionsOperation, EnableTransactionsCheck } from './enableTransactions/index.js';

const handlers = [
    { operation: 'tx', Class: TxOperation, validator: new TxCheck() },
    { operation: 'msg', Class: MsgOperation, validator: new MsgCheck() },
    { operation: 'feature', Class: FeatureOperation, validator: new FeatureCheck() },
    { operation: 'addIndexer', Class: AddIndexerOperation, validator: new AddIndexerCheck() },
    { operation: 'addWriter', Class: AddWriterOperation, validator: new AddWriterCheck() },
    { operation: 'removeWriter', Class: RemoveWriterOperation, validator: new RemoveWriterCheck() },
    { operation: 'setChatStatus', Class: SetChatStatusOperation, validator: new SetChatStatusCheck() },
    { operation: 'setAutoAddWriters', Class: SetAutoAddWritersOperation, validator: new SetAutoAddWritersCheck() },
    { operation: 'autoAddWriter', Class: AutoAddWritersOperation, validator: new AutoAddWritersCheck() },
    { operation: 'addAdmin', Class: AddAdminOperation, validator: new AddAdminCheck() },
    { operation: 'updateAdmin', Class: UpdateAdminOperation, validator: new UpdateAdminCheck() },
    { operation: 'setNick', Class: SetNickOperation, validator: new SetNickCheck() },
    { operation: 'muteStatus', Class: MuteStatusOperation, validator: new MuteStatusCheck() },
    { operation: 'deleteMessage', Class: DeleteMessageOperation, validator: new DeleteMessageCheck() },
    { operation: 'unpinMessage', Class: UnpinMessageOperation, validator: new UnpinMessageCheck() },
    { operation: 'pinMessage', Class: PinMessageOperation, validator: new PinMessageCheck() },
    { operation: 'setMod', Class: SetModOperation, validator: new SetModCheck() },
    { operation: 'setWhitelistStatus', Class: SetWhitelistStatusOperation, validator: new SetWhitelistStatusCheck() },
    { operation: 'enableWhitelist', Class: EnableWhitelistOperation, validator: new EnableWhitelistCheck() },
    { operation: 'enableTransactions', Class: EnableTransactionsOperation, validator: new EnableTransactionsCheck() }
]

export const handlerFor = (node, context) => {
    const type = node?.value?.type
    if (!type) return
    const handler = handlers.find(({ operation }) => operation === type)
    if (handler) {
        return new handler.Class(handler.validator, context)
    }
}
