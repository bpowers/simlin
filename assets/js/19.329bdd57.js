(window.webpackJsonp=window.webpackJsonp||[]).push([[19],{102:function(e,t){function a(e){let t,a=[];for(let n of e.split(",").map((e=>e.trim())))if(/^-?\d+$/.test(n))a.push(parseInt(n,10));else if(t=n.match(/^(-?\d+)(-|\.\.\.?|\u2025|\u2026|\u22EF)(-?\d+)$/)){let[e,n,l,r]=t;if(n&&r){n=parseInt(n),r=parseInt(r);const e=n<r?1:-1;"-"!==l&&".."!==l&&"\u2025"!==l||(r+=e);for(let t=n;t!==r;t+=e)a.push(t)}}return a}t.default=a,e.exports=a},104:function(e,t,a){"use strict";var n=a(0),l=a.n(n),r=a(96),o=a(3),s=a(94),i={plain:{backgroundColor:"#2a2734",color:"#9a86fd"},styles:[{types:["comment","prolog","doctype","cdata","punctuation"],style:{color:"#6c6783"}},{types:["namespace"],style:{opacity:.7}},{types:["tag","operator","number"],style:{color:"#e09142"}},{types:["property","function"],style:{color:"#9a86fd"}},{types:["tag-id","selector","atrule-id"],style:{color:"#eeebff"}},{types:["attr-name"],style:{color:"#c4b9fe"}},{types:["boolean","string","entity","url","attr-value","keyword","control","directive","unit","statement","regex","at-rule","placeholder","variable"],style:{color:"#ffcc99"}},{types:["deleted"],style:{textDecorationLine:"line-through"}},{types:["inserted"],style:{textDecorationLine:"underline"}},{types:["italic"],style:{fontStyle:"italic"}},{types:["important","bold"],style:{fontWeight:"bold"}},{types:["important"],style:{color:"#c4b9fe"}}]},c={Prism:a(23).a,theme:i};function m(e,t,a){return t in e?Object.defineProperty(e,t,{value:a,enumerable:!0,configurable:!0,writable:!0}):e[t]=a,e}function p(){return(p=Object.assign||function(e){for(var t=1;t<arguments.length;t++){var a=arguments[t];for(var n in a)Object.prototype.hasOwnProperty.call(a,n)&&(e[n]=a[n])}return e}).apply(this,arguments)}var d=/\r\n|\r|\n/,u=function(e){0===e.length?e.push({types:["plain"],content:"\n",empty:!0}):1===e.length&&""===e[0].content&&(e[0].content="\n",e[0].empty=!0)},g=function(e,t){var a=e.length;return a>0&&e[a-1]===t?e:e.concat(t)},h=function(e,t){var a=e.plain,n=Object.create(null),l=e.styles.reduce((function(e,a){var n=a.languages,l=a.style;return n&&!n.includes(t)||a.types.forEach((function(t){var a=p({},e[t],l);e[t]=a})),e}),n);return l.root=a,l.plain=p({},a,{backgroundColor:null}),l};function y(e,t){var a={};for(var n in e)Object.prototype.hasOwnProperty.call(e,n)&&-1===t.indexOf(n)&&(a[n]=e[n]);return a}var b=function(e){function t(){for(var t=this,a=[],n=arguments.length;n--;)a[n]=arguments[n];e.apply(this,a),m(this,"getThemeDict",(function(e){if(void 0!==t.themeDict&&e.theme===t.prevTheme&&e.language===t.prevLanguage)return t.themeDict;t.prevTheme=e.theme,t.prevLanguage=e.language;var a=e.theme?h(e.theme,e.language):void 0;return t.themeDict=a})),m(this,"getLineProps",(function(e){var a=e.key,n=e.className,l=e.style,r=p({},y(e,["key","className","style","line"]),{className:"token-line",style:void 0,key:void 0}),o=t.getThemeDict(t.props);return void 0!==o&&(r.style=o.plain),void 0!==l&&(r.style=void 0!==r.style?p({},r.style,l):l),void 0!==a&&(r.key=a),n&&(r.className+=" "+n),r})),m(this,"getStyleForToken",(function(e){var a=e.types,n=e.empty,l=a.length,r=t.getThemeDict(t.props);if(void 0!==r){if(1===l&&"plain"===a[0])return n?{display:"inline-block"}:void 0;if(1===l&&!n)return r[a[0]];var o=n?{display:"inline-block"}:{},s=a.map((function(e){return r[e]}));return Object.assign.apply(Object,[o].concat(s))}})),m(this,"getTokenProps",(function(e){var a=e.key,n=e.className,l=e.style,r=e.token,o=p({},y(e,["key","className","style","token"]),{className:"token "+r.types.join(" "),children:r.content,style:t.getStyleForToken(r),key:void 0});return void 0!==l&&(o.style=void 0!==o.style?p({},o.style,l):l),void 0!==a&&(o.key=a),n&&(o.className+=" "+n),o})),m(this,"tokenize",(function(e,t,a,n){var l={code:t,grammar:a,language:n,tokens:[]};e.hooks.run("before-tokenize",l);var r=l.tokens=e.tokenize(l.code,l.grammar,l.language);return e.hooks.run("after-tokenize",l),r}))}return e&&(t.__proto__=e),t.prototype=Object.create(e&&e.prototype),t.prototype.constructor=t,t.prototype.render=function(){var e=this.props,t=e.Prism,a=e.language,n=e.code,l=e.children,r=this.getThemeDict(this.props),o=t.languages[a];return l({tokens:function(e){for(var t=[[]],a=[e],n=[0],l=[e.length],r=0,o=0,s=[],i=[s];o>-1;){for(;(r=n[o]++)<l[o];){var c=void 0,m=t[o],p=a[o][r];if("string"==typeof p?(m=o>0?m:["plain"],c=p):(m=g(m,p.type),p.alias&&(m=g(m,p.alias)),c=p.content),"string"==typeof c){var h=c.split(d),y=h.length;s.push({types:m,content:h[0]});for(var b=1;b<y;b++)u(s),i.push(s=[]),s.push({types:m,content:h[b]})}else o++,t.push(m),a.push(c),n.push(0),l.push(c.length)}o--,t.pop(),a.pop(),n.pop(),l.pop()}return u(s),i}(void 0!==o?this.tokenize(t,n,o,a):[n]),className:"prism-code language-"+a,style:void 0!==r?r.root:{},getLineProps:this.getLineProps,getTokenProps:this.getTokenProps})},t}(n.Component);var v=a(102),f=a.n(v),k={plain:{color:"#bfc7d5",backgroundColor:"#292d3e"},styles:[{types:["comment"],style:{color:"rgb(105, 112, 152)",fontStyle:"italic"}},{types:["string","inserted"],style:{color:"rgb(195, 232, 141)"}},{types:["number"],style:{color:"rgb(247, 140, 108)"}},{types:["builtin","char","constant","function"],style:{color:"rgb(130, 170, 255)"}},{types:["punctuation","selector"],style:{color:"rgb(199, 146, 234)"}},{types:["variable"],style:{color:"rgb(191, 199, 213)"}},{types:["class-name","attr-name"],style:{color:"rgb(255, 203, 107)"}},{types:["tag","deleted"],style:{color:"rgb(255, 85, 114)"}},{types:["operator"],style:{color:"rgb(137, 221, 255)"}},{types:["boolean"],style:{color:"rgb(255, 88, 116)"}},{types:["keyword"],style:{fontStyle:"italic"}},{types:["doctype"],style:{color:"rgb(199, 146, 234)",fontStyle:"italic"}},{types:["namespace"],style:{color:"rgb(178, 204, 214)"}},{types:["url"],style:{color:"rgb(221, 221, 221)"}}]},E=a(106),N=a(93);var j=()=>{const{prism:e}=Object(N.useThemeConfig)(),{isDarkTheme:t}=Object(E.a)(),a=e.theme||k,n=e.darkTheme||a;return t?n:a},T=a(95),O=a(56),_=a.n(O);const w=/{([\d,-]+)}/,x=(e=["js","jsBlock","jsx","python","html"])=>{const t={js:{start:"\\/\\/",end:""},jsBlock:{start:"\\/\\*",end:"\\*\\/"},jsx:{start:"\\{\\s*\\/\\*",end:"\\*\\/\\s*\\}"},python:{start:"#",end:""},html:{start:"\x3c!--",end:"--\x3e"}},a=["highlight-next-line","highlight-start","highlight-end"].join("|"),n=e.map((e=>`(?:${t[e].start}\\s*(${a})\\s*${t[e].end})`)).join("|");return new RegExp(`^\\s*(?:${n})\\s*$`)};function C({children:e,className:t,metastring:a,title:r}){const{prism:i}=Object(N.useThemeConfig)(),[m,p]=Object(n.useState)(!1),[d,u]=Object(n.useState)(!1);Object(n.useEffect)((()=>{u(!0)}),[]);const g=Object(N.parseCodeBlockTitle)(a)||r,h=Object(n.useRef)(null);let y=[];const v=j(),k=Array.isArray(e)?e.join(""):e;if(a&&w.test(a)){const e=a.match(w)[1];y=f()(e).filter((e=>e>0))}let E=t&&t.replace(/language-/,"");!E&&i.defaultLanguage&&(E=i.defaultLanguage);let O=k.replace(/\n$/,"");if(0===y.length&&void 0!==E){let e="";const t=(e=>{switch(e){case"js":case"javascript":case"ts":case"typescript":return x(["js","jsBlock"]);case"jsx":case"tsx":return x(["js","jsBlock","jsx"]);case"html":return x(["js","jsBlock","html"]);case"python":case"py":return x(["python"]);default:return x()}})(E),a=k.replace(/\n$/,"").split("\n");let n;for(let l=0;l<a.length;){const r=l+1,o=a[l].match(t);if(null!==o){switch(o.slice(1).reduce(((e,t)=>e||t),void 0)){case"highlight-next-line":e+=`${r},`;break;case"highlight-start":n=r;break;case"highlight-end":e+=`${n}-${r-1},`}a.splice(l,1)}else l+=1}y=f()(e),O=a.join("\n")}const C=()=>{!function(e,{target:t=document.body}={}){const a=document.createElement("textarea"),n=document.activeElement;a.value=e,a.setAttribute("readonly",""),a.style.contain="strict",a.style.position="absolute",a.style.left="-9999px",a.style.fontSize="12pt";const l=document.getSelection();let r=!1;l.rangeCount>0&&(r=l.getRangeAt(0)),t.append(a),a.select(),a.selectionStart=0,a.selectionEnd=e.length;let o=!1;try{o=document.execCommand("copy")}catch{}a.remove(),r&&(l.removeAllRanges(),l.addRange(r)),n&&n.focus()}(O),p(!0),setTimeout((()=>p(!1)),2e3)};return l.a.createElement(b,Object(o.a)({},c,{key:String(d),theme:v,code:O,language:E}),(({className:e,style:t,tokens:a,getLineProps:n,getTokenProps:r})=>l.a.createElement("div",{className:_.a.codeBlockContainer},g&&l.a.createElement("div",{style:t,className:_.a.codeBlockTitle},g),l.a.createElement("div",{className:Object(s.a)(_.a.codeBlockContent,E)},l.a.createElement("div",{tabIndex:0,className:Object(s.a)(e,_.a.codeBlock,"thin-scrollbar",{[_.a.codeBlockWithTitle]:g})},l.a.createElement("div",{className:_.a.codeBlockLines,style:t},a.map(((e,t)=>{1===e.length&&""===e[0].content&&(e[0].content="\n");const a=n({line:e,key:t});return y.includes(t+1)&&(a.className=`${a.className} docusaurus-highlight-code-line`),l.a.createElement("div",Object(o.a)({key:t},a),e.map(((e,t)=>l.a.createElement("span",Object(o.a)({key:t},r({token:e,key:t}))))))})))),l.a.createElement("button",{ref:h,type:"button","aria-label":Object(T.b)({id:"theme.CodeBlock.copyButtonAriaLabel",message:"Copy code to clipboard",description:"The ARIA label for copy code blocks button"}),className:Object(s.a)(_.a.copyButton),onClick:C},m?l.a.createElement(T.a,{id:"theme.CodeBlock.copied",description:"The copied button label on code blocks"},"Copied"):l.a.createElement(T.a,{id:"theme.CodeBlock.copy",description:"The copy button label on code blocks"},"Copy"))))))}a(57);var L=a(58),P=a.n(L);var B=e=>function({id:t,...a}){const{navbar:{hideOnScroll:n}}=Object(N.useThemeConfig)();return t?l.a.createElement(e,a,l.a.createElement("a",{"aria-hidden":"true",tabIndex:-1,className:Object(s.a)("anchor",{[P.a.enhancedAnchor]:!n}),id:t}),a.children,l.a.createElement("a",{className:"hash-link",href:`#${t}`,title:Object(T.b)({id:"theme.common.headingLinkTitle",message:"Direct link to heading",description:"Title for link to heading"})},"#")):l.a.createElement(e,a)};const $={code:e=>{const{children:t}=e;return Object(n.isValidElement)(t)?t:t.includes("\n")?l.a.createElement(C,e):l.a.createElement("code",e)},a:e=>l.a.createElement(r.a,e),pre:e=>{var t;const{children:a}=e;return Object(n.isValidElement)(null==a||null===(t=a.props)||void 0===t?void 0:t.children)?null==a?void 0:a.props.children:l.a.createElement(C,Object(n.isValidElement)(a)?null==a?void 0:a.props:{children:a})},h1:B("h1"),h2:B("h2"),h3:B("h3"),h4:B("h4"),h5:B("h5"),h6:B("h6")};t.a=$},115:function(e,t,a){"use strict";var n=a(0),l=a.n(n),r=a(94),o=a(99),s=a(95),i=a(96),c=a(104),m=a(110),p=a(61),d=a.n(p),u=a(93);t.a=function(e){const t=function(){const{selectMessage:e}=Object(u.usePluralForm)();return t=>{const a=Math.ceil(t);return e(a,Object(s.b)({id:"theme.blog.post.readingTime.plurals",description:'Pluralized label for "{readingTime} min read". Use as much plural forms (separated by "|") as your language support (see https://www.unicode.org/cldr/cldr-aux/charts/34/supplemental/language_plural_rules.html)',message:"One min read|{readingTime} min read"},{readingTime:a}))}}(),{children:a,frontMatter:n,metadata:p,truncated:g,isBlogPostPage:h=!1}=e,{date:y,formattedDate:b,permalink:v,tags:f,readingTime:k}=p,{author:E,title:N,image:j,keywords:T}=n,O=n.author_url||n.authorURL,_=n.author_title||n.authorTitle,w=n.author_image_url||n.authorImageURL;return l.a.createElement(l.a.Fragment,null,l.a.createElement(m.a,{keywords:T,image:j}),l.a.createElement("article",{className:h?void 0:"margin-bottom--xl"},(()=>{const e=h?"h1":"h2";return l.a.createElement("header",null,l.a.createElement(e,{className:Object(r.a)("margin-bottom--sm",d.a.blogPostTitle)},h?N:l.a.createElement(i.a,{to:v},N)),l.a.createElement("div",{className:"margin-vert--md"},l.a.createElement("time",{dateTime:y,className:d.a.blogPostDate},b,k&&l.a.createElement(l.a.Fragment,null," \xb7 ",t(k)))),l.a.createElement("div",{className:"avatar margin-vert--md"},w&&l.a.createElement(i.a,{className:"avatar__photo-link avatar__photo",href:O},l.a.createElement("img",{src:w,alt:E})),l.a.createElement("div",{className:"avatar__intro"},E&&l.a.createElement(l.a.Fragment,null,l.a.createElement("h4",{className:"avatar__name"},l.a.createElement(i.a,{href:O},E)),l.a.createElement("small",{className:"avatar__subtitle"},_)))))})(),l.a.createElement("div",{className:"markdown"},l.a.createElement(o.a,{components:c.a},a)),(f.length>0||g)&&l.a.createElement("footer",{className:"row margin-vert--lg"},f.length>0&&l.a.createElement("div",{className:"col"},l.a.createElement("strong",null,l.a.createElement(s.a,{id:"theme.tags.tagsListLabel",description:"The label alongside a tag list"},"Tags:")),f.map((({label:e,permalink:t})=>l.a.createElement(i.a,{key:t,className:"margin-horiz--sm",to:t},e)))),g&&l.a.createElement("div",{className:"col text--right"},l.a.createElement(i.a,{to:p.permalink,"aria-label":`Read more about ${N}`},l.a.createElement("strong",null,l.a.createElement(s.a,{id:"theme.blog.post.readMore",description:"The label used in blog post item excerpts to link to full blog posts"},"Read More")))))))}},116:function(e,t,a){"use strict";a.d(t,"a",(function(){return c}));var n=a(0),l=a.n(n),r=a(94),o=a(96),s=a(62),i=a.n(s);function c({sidebar:e}){return 0===e.items.length?null:l.a.createElement("div",{className:Object(r.a)(i.a.sidebar,"thin-scrollbar")},l.a.createElement("h3",{className:i.a.sidebarItemTitle},e.title),l.a.createElement("ul",{className:i.a.sidebarItemList},e.items.map((e=>l.a.createElement("li",{key:e.permalink,className:i.a.sidebarItem},l.a.createElement(o.a,{isNavLink:!0,to:e.permalink,className:i.a.sidebarItemLink,activeClassName:i.a.sidebarItemLinkActive},e.title))))))}},91:function(e,t,a){"use strict";a.r(t);var n=a(0),l=a.n(n),r=a(16),o=a(103),s=a(115),i=a(96),c=a(95);var m=function(e){const{metadata:t}=e,{previousPage:a,nextPage:n}=t;return l.a.createElement("nav",{className:"pagination-nav","aria-label":Object(c.b)({id:"theme.blog.paginator.navAriaLabel",message:"Blog list page navigation",description:"The ARIA label for the blog pagination"})},l.a.createElement("div",{className:"pagination-nav__item"},a&&l.a.createElement(i.a,{className:"pagination-nav__link",to:a},l.a.createElement("div",{className:"pagination-nav__label"},"\xab"," ",l.a.createElement(c.a,{id:"theme.blog.paginator.newerEntries",description:"The label used to navigate to the newer blog posts page (previous page)"},"Newer Entries")))),l.a.createElement("div",{className:"pagination-nav__item pagination-nav__item--next"},n&&l.a.createElement(i.a,{className:"pagination-nav__link",to:n},l.a.createElement("div",{className:"pagination-nav__label"},l.a.createElement(c.a,{id:"theme.blog.paginator.olderEntries",description:"The label used to navigate to the older blog posts page (next page)"},"Older Entries")," ","\xbb"))))},p=a(116),d=a(93);t.default=function(e){const{metadata:t,items:a,sidebar:n}=e,{siteConfig:{title:i}}=Object(r.default)(),{blogDescription:c,blogTitle:u,permalink:g}=t,h="/"===g?i:u;return l.a.createElement(o.a,{title:h,description:c,wrapperClassName:d.ThemeClassNames.wrapper.blogPages,pageClassName:d.ThemeClassNames.page.blogListPage,searchMetadatas:{tag:"blog_posts_list"}},l.a.createElement("div",{className:"container margin-vert--lg"},l.a.createElement("div",{className:"row"},l.a.createElement("div",{className:"col col--3"},l.a.createElement(p.a,{sidebar:n})),l.a.createElement("main",{className:"col col--7"},a.map((({content:e})=>l.a.createElement(s.a,{key:e.metadata.permalink,frontMatter:e.frontMatter,metadata:e.metadata,truncated:e.metadata.truncated},l.a.createElement(e,null)))),l.a.createElement(m,{metadata:t})))))}}}]);