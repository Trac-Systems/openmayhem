import BaseContract from "../src/artifacts/contract.js";

export const GEN1_CREATURES = [
  { id: 1, name: "Leafling" },
  { id: 2, name: "Verdelon" },
  { id: 3, name: "Florazar" },
  { id: 4, name: "Emberbit" },
  { id: 5, name: "Flarecub" },
  { id: 6, name: "Pyronyx" },
  { id: 7, name: "Splashit" },
  { id: 8, name: "Aquafin" },
  { id: 9, name: "Torrento" },
  { id: 10, name: "Silkwormy" },
  { id: 11, name: "Cocoonix" },
  { id: 12, name: "Fluttera" },
  { id: 13, name: "Needlit" },
  { id: 14, name: "Spindle" },
  { id: 15, name: "Stinglade" },
  { id: 16, name: "Breezlet" },
  { id: 17, name: "Skyrift" },
  { id: 18, name: "Aerolynx" },
  { id: 19, name: "Nibblemouse" },
  { id: 20, name: "Gnawfang" },
  { id: 21, name: "Pecklet" },
  { id: 22, name: "Razorwing" },
  { id: 23, name: "Coilisk" },
  { id: 24, name: "Venobra" },
  { id: 25, name: "Sparkit" },
  { id: 26, name: "Voltiger" },
  { id: 27, name: "Dunepup" },
  { id: 28, name: "Sandclaw" },
  { id: 29, name: "Nidelle" },
  { id: 30, name: "Nidora" },
  { id: 31, name: "Regalynx" },
  { id: 32, name: "Nidorn" },
  { id: 33, name: "Ravihorn" },
  { id: 34, name: "Warcrest" },
  { id: 35, name: "Moonbit" },
  { id: 36, name: "Lunabelle" },
  { id: 37, name: "Emberfox" },
  { id: 38, name: "Pyrevale" },
  { id: 39, name: "Humchime" },
  { id: 40, name: "Choiruff" },
  { id: 41, name: "Batnip" },
  { id: 42, name: "Noctalon" },
  { id: 43, name: "Sproutle" },
  { id: 44, name: "Murkbloom" },
  { id: 45, name: "Petalord" },
  { id: 46, name: "Shroomit" },
  { id: 47, name: "Fungarok" },
  { id: 48, name: "Buzzeye" },
  { id: 49, name: "Mothveil" },
  { id: 50, name: "Burrowl" },
  { id: 51, name: "Triholem" },
  { id: 52, name: "Whiskit" },
  { id: 53, name: "Silkclaw" },
  { id: 54, name: "Quaxel" },
  { id: 55, name: "Hydrake" },
  { id: 56, name: "Ragekit" },
  { id: 57, name: "Furyape" },
  { id: 58, name: "Cinderpup" },
  { id: 59, name: "Blazehound" },
  { id: 60, name: "Ripplet" },
  { id: 61, name: "Spiralswim" },
  { id: 62, name: "Tidemaster" },
  { id: 63, name: "Mindlet" },
  { id: 64, name: "Psycrux" },
  { id: 65, name: "Neuragon" },
  { id: 66, name: "Fistling" },
  { id: 67, name: "Brawlorn" },
  { id: 68, name: "Titanuckle" },
  { id: 69, name: "Snapvine" },
  { id: 70, name: "Clampod" },
  { id: 71, name: "Venivine" },
  { id: 72, name: "Jellish" },
  { id: 73, name: "Abysslime" },
  { id: 74, name: "Rocknub" },
  { id: 75, name: "Stonehaul" },
  { id: 76, name: "Granitus" },
  { id: 77, name: "Flametail" },
  { id: 78, name: "Infermare" },
  { id: 79, name: "Drowsnout" },
  { id: 80, name: "Mindreef" },
  { id: 81, name: "Magnetik" },
  { id: 82, name: "Polaron" },
  { id: 83, name: "Bladebeak" },
  { id: 84, name: "Twinbeak" },
  { id: 85, name: "Triplume" },
  { id: 86, name: "Frostell" },
  { id: 87, name: "Glaciark" },
  { id: 88, name: "Sludgit" },
  { id: 89, name: "Toximass" },
  { id: 90, name: "Shellbit" },
  { id: 91, name: "Cryoclash" },
  { id: 92, name: "Wispel" },
  { id: 93, name: "Phantor" },
  { id: 94, name: "Dreadveil" },
  { id: 95, name: "Ironcoil" },
  { id: 96, name: "Somnix" },
  { id: 97, name: "Nightseer" },
  { id: 98, name: "Clawfin" },
  { id: 99, name: "Crustazor" },
  { id: 100, name: "Zaporb" },
  { id: 101, name: "Voltcore" },
  { id: 102, name: "Seedpod" },
  { id: 103, name: "Palmind" },
  { id: 104, name: "Boneling" },
  { id: 105, name: "Skullmaul" },
  { id: 106, name: "Kicklash" },
  { id: 107, name: "Punchflare" },
  { id: 108, name: "Slurpent" },
  { id: 109, name: "Smoggit" },
  { id: 110, name: "Fumegon" },
  { id: 111, name: "Ramhorn" },
  { id: 112, name: "Ironstamp" },
  { id: 113, name: "Healfawn" },
  { id: 114, name: "Vinetangle" },
  { id: 115, name: "Pouchlord" },
  { id: 116, name: "Seaspark" },
  { id: 117, name: "Maridra" },
  { id: 118, name: "Glimfin" },
  { id: 119, name: "Crestide" },
  { id: 120, name: "Starbud" },
  { id: 121, name: "Astrion" },
  { id: 122, name: "Mimeveil" },
  { id: 123, name: "Bladewing" },
  { id: 124, name: "Frosthex" },
  { id: 125, name: "Sparkhorn" },
  { id: 126, name: "Pyrofang" },
  { id: 127, name: "Griptor" },
  { id: 128, name: "Stampede" },
  { id: 129, name: "Flopscale" },
  { id: 130, name: "Riptalon" },
  { id: 131, name: "Glacieron" },
  { id: 132, name: "Morphix" },
  { id: 133, name: "Adaptle" },
  { id: 134, name: "Aquarion" },
  { id: 135, name: "Voltair" },
  { id: 136, name: "Ignion" },
  { id: 137, name: "Databit" },
  { id: 138, name: "Shellith" },
  { id: 139, name: "Reeflord" },
  { id: 140, name: "Carapup" },
  { id: 141, name: "Scyrox" },
  { id: 142, name: "Skydactyl" },
  { id: 143, name: "Snorbeast" },
  { id: 144, name: "Cryostorm" },
  { id: 145, name: "Thunderra" },
  { id: 146, name: "Solarion" },
  { id: 147, name: "Draklet" },
  { id: 148, name: "Aerialisk" },
  { id: 149, name: "Dracoryx" },
  { id: 150, name: "Psyrex" },
  { id: 151, name: "Eonling" },
];

class TuxemonContract extends BaseContract {
  constructor(protocol, config) {
    super(protocol, config);
    this.addFunction("catch");
  }

  async catch() {
    const { tuxemons } = await this.#ensureTracdex()
    const parsed = JSON.parse(tuxemons)

    const pick = GEN1_CREATURES[Math.floor(Math.random() * GEN1_CREATURES.length)];

    const exists = parsed.some(p => p.id === pick.id);
    if (!exists) parsed.push({ id: pick.id, name: pick.name, tx: this.tx });

    await this.#setTracdex(parsed)
  }

  async #ensureTracdex() {
    const tuxedex = await this.get(this.#tracdexKey())
    if (tuxedex === null) {
      await this.#setTracdex([])
    }

    return await this.get(this.#tracdexKey())
  }

  #tracdexKey() {
    return `app/tuxedex/${this.address}`
  }

  async #setTracdex(tuxemons) {
    await this.put(this.#tracdexKey(), { tuxemons: JSON.stringify(tuxemons) });
  }
}

export default TuxemonContract;
