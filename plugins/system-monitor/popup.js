const defaults={cpu:true,temperature:true,frequency:true,logical:true,memory:true,diskUsage:true,network:true,disk:true,fan:true,refreshMs:1000};
const toggleKeys=["cpu","temperature","frequency","logical","memory","diskUsage","network","disk","fan"];
window.tbPlugin={
  data:()=>({settings:{...defaults},snapshot:null,error:"",timer:0,refreshOptions:[{label:"1 秒",value:"1000"},{label:"5 秒",value:"5000"},{label:"10 秒",value:"10000"}]}),
  computed:{
    memoryText(){const m=this.snapshot?.memory;return m?`${this.size(m.usedBytes)} / ${this.size(m.totalBytes)}`:"--";},
    diskText(){const d=this.snapshot?.disk;return d?`${this.size(d.usedBytes)} / ${this.size(d.totalBytes)}`:"--";},
    fanText(){const fan=this.snapshot?.fans?.[0];return fan&&Number.isFinite(Number(fan.rpm))?`${Math.round(Number(fan.rpm))} RPM`:"N/A";}
  },
  methods:{
    normalize(v){
      const source=v&&typeof v==="object"?v:{};
      const refreshMs=[1000,5000,10000].includes(Number(source.refreshMs))?Number(source.refreshMs):defaults.refreshMs;
      const next={...defaults,refreshMs};
      for(const key of toggleKeys)if(typeof source[key]==="boolean")next[key]=source[key];
      return next;
    },
    pct(v){return v==null||!Number.isFinite(Number(v))?"N/A":`${Math.round(Number(v))}%`;},
    temp(v){return v==null||!Number.isFinite(Number(v))?"N/A":`${Math.round(Number(v))} °C`;},
    frequency(v){return !Number(v)?"N/A":Number(v)>=1000?`${(Number(v)/1000).toFixed(2)} GHz`:`${Math.round(Number(v))} MHz`;},
    size(v){const n=Number(v)||0;if(n>=1024**3)return`${(n/1024**3).toFixed(1)} GB`;if(n>=1024**2)return`${(n/1024**2).toFixed(1)} MB`;if(n>=1024)return`${(n/1024).toFixed(0)} KB`;return`${Math.round(n)} B`;},
    rate(v){
      const n=Math.max(0,Number(v)||0),units=["B/s","KB/s","MB/s","GB/s"];
      let i=0;while(i<units.length-1&&n>=1000*(1024**i))i++;
      const scaled=n/(1024**i);
      const amount=scaled>=100?Math.floor(scaled):scaled>=10?Math.round(scaled):scaled.toFixed(1).replace(/\.0$/,"");
      return `${amount} ${units[i]}`;
    },
    toggle(key){if(!toggleKeys.includes(key))return;void this.save({[key]:!this.settings[key]});},
    async save(patch){
      const before=this.settings;
      const previousRefresh=before.refreshMs;
      const next=this.normalize({...before,...patch});
      this.settings=next;
      try{
        await host.storage.set("settings",next);
        this.error="";
      }catch(e){
        this.error=String(e);
        this.settings=before;
        return;
      }
      if(next.refreshMs!==previousRefresh)this.restart();
    },
    async refresh(){try{this.snapshot=await host.system.metrics();this.error="";}catch(e){this.error=String(e);}},
    restart(){if(this.timer)clearInterval(this.timer);this.timer=setInterval(()=>void this.refresh(),this.settings.refreshMs);}
  },
  async mounted(){this.settings=this.normalize(await host.storage.get("settings",{}));await this.refresh();this.restart();host.ready();},
  beforeUnmount(){if(this.timer)clearInterval(this.timer);}
};
