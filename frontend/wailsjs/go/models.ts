export namespace main {
	
	export class LaunchOptions {
	    protocol: string;
	    server: string;
	    port: number;
	    username: string;
	    password: string;
	    socksBind: string;
	    httpBind: string;
	    secondaryDnsServer: string;
	    authType: string;
	    loginDomain: string;
	    clientDataFile: string;
	    eipBrowserProgram: string;
	    eipBrowserArgs: string[];
	    tunMode: boolean;
	    debugDump: boolean;
	
	    static createFrom(source: any = {}) {
	        return new LaunchOptions(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.protocol = source["protocol"];
	        this.server = source["server"];
	        this.port = source["port"];
	        this.username = source["username"];
	        this.password = source["password"];
	        this.socksBind = source["socksBind"];
	        this.httpBind = source["httpBind"];
	        this.secondaryDnsServer = source["secondaryDnsServer"];
	        this.authType = source["authType"];
	        this.loginDomain = source["loginDomain"];
	        this.clientDataFile = source["clientDataFile"];
	        this.eipBrowserProgram = source["eipBrowserProgram"];
	        this.eipBrowserArgs = source["eipBrowserArgs"];
	        this.tunMode = source["tunMode"];
	        this.debugDump = source["debugDump"];
	    }
	}

}

